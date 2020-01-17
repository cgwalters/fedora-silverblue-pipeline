use futures::prelude::*;
use rusoto_core::credential::ProfileProvider;
use rusoto_s3::{ListObjectsRequest, PutObjectRequest, S3};
use std::collections::HashSet;
use structopt::StructOpt;
//use rusoto_core::RusotoFuture;
//use futures::compat::Compat01As03;

mod cosa;

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
    let s3prefix = s3url.path().to_string();

    let s3client = rusoto_s3::S3Client::new_with(
        rusoto_core::request::HttpClient::new()?,
        ProfileProvider::new()?,
        rusoto_core::Region::UsEast1,
    );

    // TODO: This is futures01 and I couldn't get the .compat() method to work,
    // something with RusotoFuture being special?
    let current_rpms: HashSet<String> = {
        let req = ListObjectsRequest {
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
    println!("Rojig builds: {}", rojig_builds.len());

    let rojig_builds = rojig_builds;
    for build in rojig_builds.iter() {
        let rojig = build.images.rojig.as_ref().unwrap();
        if !current_rpms.contains(&rojig.path) {
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

            let req = PutObjectRequest {
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
        } else {
            println!("Already uploaded: {}", rojig.path)
        }
    }

    Ok(())
}
