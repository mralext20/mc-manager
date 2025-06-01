#[macro_use] extern crate rocket;
#[macro_use] extern crate rocket_include_static_resources;

use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use rocket::tokio::fs::{self, File};
use rocket::tokio::io::AsyncReadExt;
use rocket::Config;
use zip::write::{FileOptions, ZipWriter, ExtendedFileOptions};
use rocket::response::content::RawJson;
use rocket::tokio::fs::remove_file;
use rocket::http::Status;
use std::path::Path;
use std::env;
use serde::Deserialize;
use std::io::Write; // Needed for ZipWriter::write_all
use rocket::serde::json::{Json, json};
use regex::Regex;
use reqwest;
use rocket::data::ByteUnit;



static_response_handler! {
    "/" => index_html => "index-html",
    "/static/style.css" => style_css => "style-css",
}

#[post("/start")]
fn start() -> &'static str {
    let status = Command::new("systemctl")
        .args(["--user", "start", "atm10.service"])
        .status();
    match status {
        Ok(s) if s.success() => "Server start requested.",
        Ok(_) | Err(_) => "Failed to start server.",
    }
}

#[post("/stop")]
fn stop() -> &'static str {
    let status = Command::new("systemctl")
        .args(["--user", "stop", "atm10.service"])
        .status();
    match status {
        Ok(s) if s.success() => "Server stop requested.",
        Ok(_) | Err(_) => "Failed to stop server.",
    }
}

#[post("/restart")]
fn restart() -> &'static str {
    let status = Command::new("systemctl")
        .args(["--user", "restart", "atm10.service"])
        .status();
    match status {
        Ok(s) if s.success() => "Server restart requested.",
        Ok(_) | Err(_) => "Failed to restart server.",
    }
}

#[get("/mods.zip")]
async fn download_mods() -> Option<(rocket::http::ContentType, Vec<u8>)> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| "extra_mods".to_string());
    let mut buffer = Vec::new();
    {
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: FileOptions<ExtendedFileOptions> = FileOptions::default();
        if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if let Ok(mut f) = File::open(&path).await {
                            let mut file_buf = Vec::new();
                            if f.read_to_end(&mut file_buf).await.is_ok() {
                                let _ = writer.start_file(name, options.clone());
                                let _ = writer.write_all(&file_buf);
                            }
                        }
                    }
                }
            }
        }
        let _ = writer.finish();
    }
    Some((rocket::http::ContentType::new("application", "zip"), buffer))
}

#[get("/extra_mods_list")]
async fn extra_mods_list() -> RawJson<String> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| "extra_mods".to_string());
    let mut mods = Vec::new();
    if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    mods.push(name.to_string());
                }
            }
        }
    }
    RawJson(serde_json::to_string(&mods).unwrap())
}

#[delete("/extra_mods/<modname>")]
async fn delete_mod(modname: &str) -> &'static str {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| "extra_mods".to_string());
    let path = std::path::Path::new(&mods_dir).join(modname);
    match remove_file(&path).await {
        Ok(_) => "OK",
        Err(_) => "FAIL",
    }
}

#[post("/extra_mods_upload", data = "<data>")]
async fn extra_mods_upload(mut data: rocket::fs::TempFile<'_>) -> Result<Status, Status> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| "extra_mods".to_string());
    let filename = data.name().map(|n| n.to_string()).unwrap_or_default();
    if !filename.ends_with(".jar") {
        return Err(Status::BadRequest);
    }
    let dest = std::path::Path::new(&mods_dir).join(&filename);
    if let Err(_) = data.persist_to(&dest).await {
        return Err(Status::InternalServerError);
    }
    Ok(Status::Ok)
}

#[derive(Debug, Deserialize)]
pub struct ModEntry {
    pub file: String,
    // Add other fields if needed
}

#[derive(Debug, Deserialize)]
pub struct Modlist {
    pub mods: Vec<ModEntry>,
}

#[post("/update_extras")]
async fn update_extras() -> Status {
    // 1. Stop the server
    let stop_status = Command::new("systemctl")
        .args(["--user", "stop", "atm10.service"])
        .status();
    if !matches!(stop_status, Ok(s) if s.success()) {
        return Status::InternalServerError;
    }

    // 2. Determine server location
    let server_location = env::var("SERVER_LOCATION").unwrap_or_else(|_| "atm10".to_string());
    let mods_dir = Path::new(&server_location).join("mods");
    let extra_mods_dir = env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| "extra_mods".to_string());

    // 3. Read modlist.json from config/crash_assistant/modlist.json
    let modlist_path = Path::new("config/crash_assistant/modlist.json");
    let modlist: Option<Modlist> = match fs::read_to_string(&modlist_path).await {
        Ok(contents) => serde_json::from_str(&contents).ok(),
        Err(_) => None,
    };
    let allowed_mods: Vec<String> = match &modlist {
        Some(list) => list.mods.iter().map(|m| m.file.clone()).collect(),
        None => Vec::new(),
    };

    // 4. Delete .jar files in mods/ that are not in modlist
    if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") && !allowed_mods.contains(&name.to_string()) {
                        let _ = fs::remove_file(&path).await;
                    }
                }
            }
        }
    }

    // 5. Copy all .jar files from extra_mods to mods
    if let Ok(mut entries) = fs::read_dir(&extra_mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") {
                        let dest = mods_dir.join(name);
                        let _ = fs::copy(&path, &dest).await;
                    }
                }
            }
        }
    }

    // 6. Start the server
    let start_status = Command::new("systemctl")
        .args(["--user", "start", "atm10.service"])
        .status();
    if !matches!(start_status, Ok(s) if s.success()) {
        return Status::InternalServerError;
    }
    Status::Ok
}

#[get("/log_tail")]
async fn log_tail() -> Option<String> {
    // Use journalctl to get the last 1000 lines for the atm10.service (user scope), clean output
    let output = Command::new("journalctl")
        .args(["--user", "-u", "atm10.service", "-n", "1000", "--no-pager", "--output=cat"])
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

#[get("/check_pack_update")]
async fn check_pack_update() -> Json<serde_json::Value> {
    // 1. Read the local modpack version from $SERVER/config/bcc-common.toml
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| "atm10".to_string());
    let config_path = Path::new(&server_location).join("config/bcc-common.toml");
    let mut file = match File::open(&config_path).await {
        Ok(f) => f,
        Err(_) => return Json(json!({"error": "Could not open bcc-common.toml"})),
    };
    let mut contents = String::new();
    if file.read_to_string(&mut contents).await.is_err() {
        return Json(json!({"error": "Could not read bcc-common.toml"}));
    }
    let re = Regex::new(r#"modpackVersion\s*=\s*"([^"]+)""#).unwrap();
    let local_version = re.captures(&contents).and_then(|cap| cap.get(1)).map(|m| m.as_str().to_string());
    if local_version.is_none() {
        return Json(json!({"error": "Could not find modpackVersion in bcc-common.toml"}));
    }
    let local_version = local_version.unwrap();

    // 2. Fetch the latest modpack version from CurseForge API (files endpoint)
    let api_url = "https://www.curseforge.com/api/v1/mods/925200/files/";
    let client = reqwest::Client::new();
    let resp = match client.get(api_url)
        .header("User-Agent", "mc-manager/1.0 (https://github.com/xela/mc-manager)")
        .send().await {
        Ok(r) => r,
        Err(_) => return Json(json!({"error": "Failed to fetch CurseForge API"})),
    };
    let api_json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return Json(json!({"error": "Failed to parse CurseForge API response"})),
    };
    // Find the latest file with hasServerPack=true
    let latest_file = api_json["data"].as_array()
        .and_then(|arr| arr.iter().find(|file| file["hasServerPack"].as_bool() == Some(true)));
    let display_name = latest_file.and_then(|file| file["displayName"].as_str()).unwrap_or("");
    let version_re = Regex::new(r#"([\d.]+)"#).unwrap();
    let latest_version = version_re.captures(display_name)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    Json(json!({
        "local_version": local_version,
        "latest_version": latest_version,
        "up_to_date": local_version == latest_version
    }))
}

#[post("/update_pack")]
async fn update_pack() -> Json<serde_json::Value> {
    use rocket::tokio::fs;
    use rocket::tokio::io::AsyncWriteExt;
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    // 1. Warn user (frontend should show a warning, backend just logs)
    println!("WARNING: Running update_pack. User may lose data if not careful!");

    // 2. Define paths
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| "atm10".to_string());
    let backup_dir = format!("{}_backup", server_location);
    let files_to_backup = [
        "eula.txt",
        "ops.json",
        "server.properties",
        "config",
        "world"
    ];

    // 3. Stop the server
    let stop_status = Command::new("systemctl")
        .args(["--user", "stop", "atm10.service"])
        .status();
    if !matches!(stop_status, Ok(s) if s.success()) {
        return Json(json!({"error": "Failed to stop server"}));
    }

    // 4. Backup important files/folders
    let _ = fs::remove_dir_all(&backup_dir).await; // Clean old backup
    let _ = fs::create_dir_all(&backup_dir).await;
    for item in files_to_backup.iter() {
        let src = Path::new(&server_location).join(item);
        let dst = Path::new(&backup_dir).join(item);
        if src.exists() {
            if src.is_dir() {
                let _ = Command::new("cp").args(["-r", src.to_str().unwrap(), dst.to_str().unwrap()]).status();
            } else {
                let _ = fs::copy(&src, &dst).await;
            }
        }
    }

    // 5. Delete the server directory
    let _ = fs::remove_dir_all(&server_location).await;
    let _ = fs::create_dir_all(&server_location).await;

    // 6. Get latest server pack info from CurseForge
    let api_url = "https://www.curseforge.com/api/v1/mods/925200/files/";
    let client = reqwest::Client::new();
    let resp = match client.get(api_url)
        .header("User-Agent", "mc-manager/1.0 (https://github.com/xela/mc-manager)")
        .send().await {
        Ok(r) => r,
        Err(_) => return Json(json!({"error": "Failed to fetch CurseForge API"})),
    };
    let api_json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return Json(json!({"error": "Failed to parse CurseForge API response"})),
    };
    let latest_file = api_json["data"].as_array()
        .and_then(|arr| arr.iter().find(|file| file["hasServerPack"].as_bool() == Some(true)));
    let file_id = latest_file.and_then(|file| file["id"].as_i64()).unwrap_or(0);
    let display_name = latest_file.and_then(|file| file["displayName"].as_str()).unwrap_or("");
    let version_re = Regex::new(r#"([\d.]+)"#).unwrap();
    let latest_version = version_re.captures(display_name)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    if file_id == 0 {
        return Json(json!({"error": "Could not find latest server pack file id"}));
    }
    let download_url = format!("https://www.curseforge.com/api/v1/mods/925200/files/{}/download", file_id);
    let zip_path = Path::new(&server_location).join("server.zip");
    let mut resp = match client.get(&download_url)
        .header("User-Agent", "mc-manager/1.0 (https://github.com/xela/mc-manager)")
        .send().await {
        Ok(r) => r,
        Err(_) => return Json(json!({"error": "Failed to download server pack"})),
    };
    let mut out = match fs::File::create(&zip_path).await {
        Ok(f) => f,
        Err(_) => return Json(json!({"error": "Failed to create server.zip"})),
    };
    while let Some(chunk) = resp.chunk().await.unwrap_or(None) {
        if out.write_all(&chunk).await.is_err() {
            return Json(json!({"error": "Failed to write server.zip"}));
        }
    }

    // 7. Unzip the server pack
    let unzip_status = Command::new("unzip")
        .arg(zip_path.to_str().unwrap())
        .arg("-d")
        .arg(&server_location)
        .status();
    if !matches!(unzip_status, Ok(s) if s.success()) {
        return Json(json!({"error": "Failed to unzip server pack"}));
    }
    let _ = fs::remove_file(&zip_path).await;

    // 8. Chmod startserver.sh
    let start_script = Path::new(&server_location).join("startserver.sh");
    if start_script.exists() {
        let _ = fs::set_permissions(&start_script, Permissions::from_mode(0o755)).await;
    }

    // 9. Restore config/world/extra_mods
    for item in files_to_backup.iter() {
        let src = Path::new(&backup_dir).join(item);
        let dst = Path::new(&server_location).join(item);
        if src.exists() {
            if src.is_dir() {
                let _ = Command::new("cp").args(["-r", src.to_str().unwrap(), dst.to_str().unwrap()]).status();
            } else {
                let _ = fs::copy(&src, &dst).await;
            }
        }
    }
    // Copy extra mods
    let extra_mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| "extra_mods".to_string());
    if let Ok(mut entries) = fs::read_dir(&extra_mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") {
                        let dest = Path::new(&server_location).join("mods").join(name);
                        let _ = fs::copy(&path, &dest).await;
                    }
                }
            }
        }
    }

    // 10. Start the server
    let start_status = Command::new("systemctl")
        .args(["--user", "start", "atm10.service"])
        .status();
    if !matches!(start_status, Ok(s) if s.success()) {
        return Json(json!({"error": "Failed to start server"}));
    }

    // 11. Update motd in server.properties to the current version
    let server_properties_path = Path::new(&server_location).join("server.properties");
    if server_properties_path.exists() {
        if let Ok(contents) = fs::read_to_string(&server_properties_path).await {
            let motd_re = Regex::new(r"(?m)^motd=.*$").unwrap();
            let new_motd = format!("motd=ATM10 Server - v{}", latest_version);
            let new_contents = if motd_re.is_match(&contents) {
                motd_re.replace(&contents, new_motd.as_str()).to_string()
            } else {
                format!("{}\n{}", contents.trim_end(), new_motd)
            };
            let _ = fs::write(&server_properties_path, new_contents).await;
        }
    }

    Json(json!({"status": "Pack updated. Please verify your world and settings!"}))
}

#[launch]
fn rocket() -> rocket::Rocket<rocket::Build> {
    let mut config = Config::release_default();
    config.address = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
    config.limits = rocket::data::Limits::new()
        .limit("file", ByteUnit::Megabyte(512))
        .limit("form", ByteUnit::Megabyte(512));
    rocket::build()
        .attach(static_resources_initializer!(
            "index-html" => ("src/page", "index.html"),
            "style-css" => ("src/page", "style.css"),
        ))
        .mount("/", routes![index_html, start, stop, restart, download_mods, extra_mods_list, delete_mod, extra_mods_upload, update_extras, log_tail, check_pack_update, update_pack, style_css])
        .configure(config)
}
