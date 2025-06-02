use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ModEntry {
    pub file: String,
    // Add other fields if needed
}

#[derive(Debug, Deserialize)]
pub struct Modlist {
    pub mods: Vec<ModEntry>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct BccCommonToml {
    pub general: BccCommonGeneral,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct BccCommonGeneral {
    #[serde(rename = "modpackProjectID")]
    pub modpack_project_id: Option<u64>,
    #[serde(rename = "modpackName")]
    pub modpack_name: Option<String>,
    #[serde(rename = "modpackVersion")]
    pub modpack_version: Option<String>,
    #[serde(rename = "useMetadata")]
    pub use_metadata: Option<bool>,
}
