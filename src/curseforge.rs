use serde::{Deserialize};
use reqwest::Client;
use semver::Version;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct CurseForgeUser {
    pub id: i64,
    pub username: String,
    #[serde(rename = "twitchAvatarUrl")]
    pub twitch_avatar_url: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: String,
}
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct CurseForgeFile {
    pub id: i64,
    #[serde(rename = "dateCreated")]
    pub date_created: String,
    #[serde(rename = "dateModified")]
    pub date_modified: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "fileLength")]
    pub file_length: i64,
    #[serde(rename = "fileName")]
    pub file_name: String,
    pub status: i32,
    #[serde(rename = "projectId")]
    pub project_id: i64,
    #[serde(rename = "gameVersions")]
    pub game_versions: Vec<String>,
    #[serde(rename = "gameVersionTypeIds")]
    pub game_version_type_ids: Vec<i64>,
    #[serde(rename = "releaseType")]
    pub release_type: i32,
    #[serde(rename = "totalDownloads")]
    pub total_downloads: i64,
    pub user: CurseForgeUser,
    #[serde(rename = "additionalFilesCount")]
    pub additional_files_count: i32,
    #[serde(rename = "hasServerPack")]
    pub has_server_pack: bool,
    #[serde(rename = "additionalServerPackFilesCount")]
    pub additional_server_pack_files_count: i32,
    #[serde(rename = "isEarlyAccessContent")]
    pub is_early_access_content: bool,
    #[serde(rename = "isCompatibleWithClient")]
    pub is_compatible_with_client: bool,
}
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct CurseForgePagination {
    pub index: i32,
    #[serde(rename = "pageSize")]
    pub page_size: i32,
    #[serde(rename = "totalCount")]
    pub total_count: i32,
}
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct CurseForgeFilesResponse {
    pub data: Vec<CurseForgeFile>,
    pub pagination: CurseForgePagination,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ServerPackInfo {
    pub version: String,
}

pub async fn fetch_latest_server_pack(client: &Client) -> Result<ServerPackInfo, String> {
    let api_url = "https://www.curseforge.com/api/v1/mods/925200/files/";
    let resp = client.get(api_url)
        .header("User-Agent", "mc-manager/1.0 (https://github.com/xela/mc-manager)")
        .send().await.map_err(|_| "Failed to fetch CurseForge API".to_string())?;
    let api_json: CurseForgeFilesResponse = resp.json().await.map_err(|_| "Failed to parse CurseForge API response".to_string())?;
    let mut server_packs: Vec<_> = api_json.data.into_iter()
        .filter(|file| file.has_server_pack)
        .collect();
    server_packs.sort_by_key(|file| -file.id);
    let latest = server_packs.first().ok_or("No server pack found")?;
    // Extract version by splitting on the last '-' character
    let version_str = match latest.display_name.rsplit_once('-') {
        Some((_, v)) => v.trim(),
        None => "unknown",
    };
    // Try to parse as semver, fallback to string if not possible
    let version = Version::parse(version_str)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| version_str.to_string());
    Ok(ServerPackInfo {
        version,
    })
}
