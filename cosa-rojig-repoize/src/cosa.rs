use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct UncompressedImage {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CompressedImage {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub uncompressed_sha256: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BuildMetaImages {
    pub qemu: Option<CompressedImage>,
    pub rojig: Option<UncompressedImage>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BuildMeta {
    pub buildid: String,
    pub images: BuildMetaImages,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Build {
    pub id: String,
    pub arches: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Builds {
    pub schema_version: String,
    pub builds: Vec<Build>,
}
