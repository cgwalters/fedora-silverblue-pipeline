// Given a coreos-assembler build stream with rojig RPMs, create and manage
// an rpm-md repo of them.

// TODO: actually createrepo_c, a bit gross since we need
// to download

use futures::prelude::*;
use futures01::prelude::{Future, Stream};
use rusoto_core::credential::ProfileProvider;
use rusoto_core::RusotoError;
use rusoto_s3::S3;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use structopt::StructOpt;

mod cosa;

const STATEPATH: &'static str = "cosa-rojig-repoize-state.json";

#[derive(Debug, StructOpt)]
#[structopt(name = "cosa-rojig-repoize")]
#[structopt(rename_all = "kebab-case")]
struct Opt {
    #[structopt(long, default_value = "10")]
    history: u16,

    #[structopt(long)]
    arch: Option<String>,

    cosa_stream: String,

    s3url: String,
}

/// SyncState is stored in S3 and represents the last synchronized build.
#[derive(Debug, Serialize, Deserialize)]
struct SyncState {
    build: String,
}

async fn get_builds(baseurl: &reqwest::Url) -> Result<cosa::Builds, Box<dyn std::error::Error>> {
    let url: reqwest::Url = baseurl.join("builds.json")?;
    Ok(reqwest::get(url).await?.error_for_status()?.json().await?)
}

async fn get_build(
    baseurl: &reqwest::Url,
    id: &str,
    arch: &str,
) -> Result<cosa::BuildMeta, Box<dyn std::error::Error>> {
    let url: reqwest::Url = baseurl.join(&format!("{}/{}/meta.json", id, arch))?;
    Ok(reqwest::get(url).await?.error_for_status()?.json().await?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();
    let arch = if let Some(ref arch) = opt.arch {
        arch.to_string()
    } else {
        String::from_utf8(
            std::process::Command::new("cosa")
                .args(&["basearch"])
                .output()?
                .stdout,
        )?
    };

    let s3url = reqwest::Url::parse(&opt.s3url)?;
    let s3bucket = if let Some(host) = s3url.host() {
        host.to_string()
    } else {
        Err("Invalid s3url")?
    };
    let s3prefix = s3url.path().trim_start_matches("/").to_string();

    let s3client = rusoto_s3::S3Client::new_with(
        rusoto_core::request::HttpClient::new()?,
        ProfileProvider::new()?,
        rusoto_core::Region::UsEast1,
    );

    let current_rpms: HashSet<String> = {
        let req = rusoto_s3::ListObjectsRequest {
            bucket: s3bucket.clone(),
            prefix: Some(s3prefix.clone()),
            ..Default::default()
        };
        let res = s3client.list_objects(req).sync()?;
        let mut h = HashSet::new();
        if let Some(mut contents) = res.contents {
            for res in contents.drain(..) {
                if let Some(v) = res.key {
                    let basename = v.rsplit("/").next().expect("rsplit");
                    h.insert(basename.to_string());
                } else {
                    Err("Missing key in S3 result")?;
                }
            }
        };
        h
    };
    println!("Current RPMs: {}", current_rpms.len());

    let baseurl = reqwest::Url::parse(&opt.cosa_stream)?;
    let buildids = get_builds(&baseurl).await?;

    let mut rojig_builds = Vec::new();
    for buildid in buildids.builds.iter().take(opt.history as usize) {
        let build = get_build(&baseurl, buildid.id.as_str(), arch.as_str()).await?;
        if build.images.rojig.is_some() {
            rojig_builds.push(build);
        }
    }
    let latest_buildid = rojig_builds.last().map(|v| v.buildid.as_str());
    let latest_buildid = match latest_buildid.as_ref() {
        Some(latest_buildid) => {
            println!(
                "Total rojig builds: {} latest: {}",
                rojig_builds.len(),
                latest_buildid
            );
            latest_buildid.to_string()
        }
        None => {
            println!("No rojig builds found!");
            return Ok(());
        }
    };

    let cur_state_key = format!("{}/{}", s3prefix, STATEPATH);
    let cur_state = match s3client
        .get_object(rusoto_s3::GetObjectRequest {
            bucket: s3bucket.clone(),
            key: cur_state_key.clone(),
            ..Default::default()
        })
        .sync()
    {
        Ok(mut state) => {
            let body = state.body.take().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::Other, "request returned no body")
            })?;
            let body = body.concat2().wait()?;
            let val: SyncState = serde_json::from_slice(&body)?;
            Some(val)
        }
        Err(RusotoError::Service(rusoto_s3::GetObjectError::NoSuchKey(_))) => None,
        Err(e) => Err(e)?,
    };

    println!("Current S3 state at {} is {:?}", cur_state_key, cur_state);

    let new_rojig_builds: Vec<_> = rojig_builds
        .into_iter()
        .filter(|build| {
            let rojig = build.images.rojig.as_ref().unwrap();
            !current_rpms.contains(&rojig.path)
        })
        .collect();
    println!("New rojig builds: {}", new_rojig_builds.len());

    for build in new_rojig_builds.iter() {
        let rojig = build.images.rojig.as_ref().unwrap();
        let srcurl: reqwest::Url =
            baseurl.join(&format!("{}/{}/{}", build.buildid, arch, &rojig.path))?;
        let srcobj = reqwest::get(srcurl)
            .await?
            .error_for_status()?
            .bytes_stream();
        let srcobj = srcobj
            .map(|v| {
                v.map(|v| {
                    // Convert between bytes04 and bytes06.  :cry:
                    let mut buf = bytes04::Bytes::new();
                    buf.extend_from_slice(&v);
                    buf
                })
            })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

        let req = rusoto_s3::PutObjectRequest {
            bucket: s3bucket.clone(),
            key: format!("{}/{}", s3prefix, rojig.path),
            content_length: Some(rojig.size as i64),
            body: Some(rusoto_core::ByteStream::new(futures::compat::Compat::new(
                srcobj,
            ))),
            ..Default::default()
        };
        s3client.put_object(req).sync()?;
        println!("Uploaded: {}", rojig.path)
    }

    let dosync = if let Some(cur_state) = cur_state.as_ref() {
        cur_state.build.as_str() != latest_buildid.as_str()
    } else {
        true
    };
    if dosync {
        let new_state = SyncState {
            build: latest_buildid.to_string(),
        };
        let new_state = serde_json::to_vec(&new_state)?;
        s3client
            .put_object(rusoto_s3::PutObjectRequest {
                bucket: s3bucket.clone(),
                key: format!("{}/{}", s3prefix, STATEPATH),
                content_length: Some(new_state.len() as i64),
                body: Some(new_state.into()),
                ..Default::default()
            })
            .sync()?;
        println!("Completed sync to {}", latest_buildid)
    } else {
        println!("Already synchronized at {}", latest_buildid)
    }

    Ok(())
}
