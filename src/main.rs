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
use std::io::Write; // Needed for ZipWriter::write_all
use rocket::serde::json::{Json, json};
use rocket::data::ByteUnit;
use semver::Version;
use rocket::Request;
use rocket::form::Form;
use rocket::fs::TempFile;

mod constants;
mod serverctl;
mod curseforge;

use crate::constants::*;
use crate::serverctl::{ServerAction, systemctl_server};
use crate::curseforge::fetch_latest_server_pack;

static_response_handler! {
    "/" => index_html => "index-html",
    "/static/style.css" => style_css => "style-css",
}

#[post("/start")]
fn start() -> &'static str {
    if systemctl_server(ServerAction::Start) {
        "Server start requested."
    } else {
        "Failed to start server."
    }
}

#[post("/stop")]
fn stop() -> &'static str {
    if systemctl_server(ServerAction::Stop) {
        "Server stop requested."
    } else {
        "Failed to stop server."
    }
}

#[post("/restart")]
fn restart() -> &'static str {
    if systemctl_server(ServerAction::Restart) {
        "Server restart requested."
    } else {
        "Failed to restart server."
    }
}

#[get("/mods.zip")]
async fn download_mods() -> Result<(rocket::http::ContentType, Vec<u8>), (Status, String)> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
    let mut buffer = Vec::new();
    {
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: FileOptions<ExtendedFileOptions> = FileOptions::default();

        let mut entries = match fs::read_dir(&mods_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("Failed to read mods directory '{}': {:?}", mods_dir, e);
                return Err((Status::InternalServerError, "Failed to read mods directory.".to_string()));
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    let mut f = match File::open(&path).await {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("Failed to open file '{}': {:?}", path.display(), e);
                            continue;
                        }
                    };
                    let mut file_buf = Vec::new();
                    if let Err(e) = f.read_to_end(&mut file_buf).await {
                        eprintln!("Failed to read file '{}': {:?}", path.display(), e);
                        continue;
                    }
                    if let Err(e) = writer.start_file(name, options.clone()) {
                        eprintln!("Failed to start zip entry for '{}': {:?}", name, e);
                        continue;
                    }
                    if let Err(e) = writer.write_all(&file_buf) {
                        eprintln!("Failed to write to zip entry for '{}': {:?}", name, e);
                        continue;
                    }
                }
            }
        }

        if let Err(e) = writer.finish() {
            eprintln!("Failed to finalize zip archive: {:?}", e);
            return Err((Status::InternalServerError, "Failed to create zip archive.".to_string()));
        }
    }
    Ok((rocket::http::ContentType::new("application", "zip"), buffer))
}

#[get("/extra_mods_list")]
async fn extra_mods_list() -> Result<RawJson<String>, (Status, String)> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
    let mut mods = Vec::new();

    let mut entries = match fs::read_dir(&mods_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Failed to read mods directory '{}': {:?}", mods_dir, e);
            return Err((Status::InternalServerError, "Failed to read mods directory.".to_string()));
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                mods.push(name.to_string());
            }
        }
    }

    match serde_json::to_string(&mods) {
        Ok(json) => Ok(RawJson(json)),
        Err(e) => {
            eprintln!("Failed to serialize mod list: {:?}", e);
            Err((Status::InternalServerError, "Failed to serialize mod list.".to_string()))
        }
    }
}

#[delete("/extra_mods/<modname>")]
async fn delete_mod(modname: &str) -> Result<Status, (Status, String)> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
    let path = std::path::Path::new(&mods_dir).join(modname);
    match remove_file(&path).await {
        Ok(_) => Ok(Status::Ok),
        Err(e) => {
            eprintln!("Failed to delete mod '{}': {:?}", modname, e);
            Err((Status::InternalServerError, format!("Failed to delete mod: {}", e)))
        }
    }
}

#[derive(FromForm)]
pub struct ModUpload<'r> {
    mod_file: TempFile<'r>,
}

#[post("/extra_mods_upload", data = "<form>")]
async fn extra_mods_upload(mut form: Form<ModUpload<'_>>) -> Result<Status, (Status, String)> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
    let mod_file = &mut form.mod_file;

    let filename = match mod_file.name() {
        Some(name) => name.to_string(),
        None => {
            return Err((Status::BadRequest, "File is missing a filename.".to_string()));
        }
    };

    // Sanitize the filename to prevent path traversal attacks.
    let sanitized_filename = std::path::Path::new(&filename)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    if sanitized_filename.is_empty() {
        return Err((Status::BadRequest, "Filename is empty or invalid.".to_string()));
    }

    if !sanitized_filename.ends_with(".jar") {
        return Err((Status::BadRequest, "Invalid file type. Only .jar files are allowed.".to_string()));
    }

    if let Err(e) = fs::create_dir_all(&mods_dir).await {
        eprintln!("Failed to create mods directory '{}': {:?}", mods_dir, e);
        return Err((Status::InternalServerError, "Failed to create mods directory.".to_string()));
    }

    let dest_path = std::path::Path::new(&mods_dir).join(&sanitized_filename);

    match mod_file.copy_to(&dest_path).await {
        Ok(_) => {
            println!("Successfully saved mod to: {}", dest_path.display());
            Ok(Status::Ok)
        }
        Err(e) => {
            eprintln!("Failed to write uploaded file '{}' to '{}': {:?}", sanitized_filename, dest_path.display(), e);
            Err((Status::InternalServerError, "Failed to save uploaded file.".to_string()))
        }
    }
}

#[post("/backup_server")]
async fn backup_server() -> Result<Json<serde_json::Value>, (Status, String)> {
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let backup_dir = format!("{}_backup", server_location);
    let files_to_backup = FILES_TO_BACKUP;

    if let Err(e) = fs::remove_dir_all(&backup_dir).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("Failed to remove existing backup directory: {:?}", e);
            return Err((Status::InternalServerError, "Failed to remove existing backup directory.".to_string()));
        }
    }

    if let Err(e) = fs::create_dir_all(&backup_dir).await {
        eprintln!("Failed to create backup directory: {:?}", e);
        return Err((Status::InternalServerError, "Failed to create backup directory.".to_string()));
    }

    let mods_dir = std::path::Path::new(&server_location).join("mods");
    let mods_list_path = std::path::Path::new(&server_location).join("mods.list");

    if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
        let mut mod_names = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") {
                        mod_names.push(name.to_string());
                    }
                }
            }
        }
        let mods_list_content = mod_names.join("\n");
        if let Err(e) = fs::write(&mods_list_path, mods_list_content).await {
            eprintln!("Failed to write mods.list: {:?}", e);
            return Err((Status::InternalServerError, "Failed to write mods.list.".to_string()));
        }
    }

    for item in files_to_backup.iter() {
        let src = std::path::Path::new(&server_location).join(item);
        let dst = std::path::Path::new(&backup_dir).join(item);
        if src.exists() {
            if src.is_dir() {
                let status = Command::new("cp").args(["-r", src.to_str().unwrap(), dst.to_str().unwrap()]).status();
                if !status.map_or(false, |s| s.success()) {
                    eprintln!("Failed to copy directory from {} to {}", src.display(), dst.display());
                    return Err((Status::InternalServerError, "Failed to copy directory.".to_string()));
                }
            } else {
                if let Err(e) = fs::copy(&src, &dst).await {
                    eprintln!("Failed to copy file from {} to {}: {:?}", src.display(), dst.display(), e);
                    return Err((Status::InternalServerError, "Failed to copy file.".to_string()));
                }
            }
        }
    }

    Ok(Json(json!({"status": "Backup complete"})))
}

#[post("/restore_server")]
async fn restore_server() -> Result<Json<serde_json::Value>, (Status, String)> {
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let backup_dir = format!("{}_backup", server_location);
    let files_to_backup = FILES_TO_BACKUP;

    for item in files_to_backup.iter() {
        let src = std::path::Path::new(&backup_dir).join(item);
        let dst = std::path::Path::new(&server_location).join(item);
        if src.exists() {
            if src.is_dir() {
                let status = Command::new("cp").args(["-r", src.to_str().unwrap(), dst.to_str().unwrap()]).status();
                if !status.map_or(false, |s| s.success()) {
                    eprintln!("Failed to copy directory from {} to {}", src.display(), dst.display());
                    return Err((Status::InternalServerError, "Failed to copy directory.".to_string()));
                }
            } else {
                if let Err(e) = fs::copy(&src, &dst).await {
                    eprintln!("Failed to copy file from {} to {}: {:?}", src.display(), dst.display(), e);
                    return Err((Status::InternalServerError, "Failed to copy file.".to_string()));
                }
            }
        }
    }

    let config_path = std::path::Path::new(&server_location).join("config/bcc-common.toml");
    let version = if let Ok(mut file) = File::open(&config_path).await {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).await.is_ok() {
            let re = regex::Regex::new(r#"modpackVersion\s*=\s*\"([^\"]*)\""#).unwrap();
            re.captures(&contents).and_then(|cap| cap.get(1)).map(|m| m.as_str().to_string())
        } else { None }
    } else { None };

    let server_properties_path = std::path::Path::new(&server_location).join("server.properties");
    if let Some(version_val) = version {
        let motd_val = format!("V{} + extras", version_val);
        if let Ok(mut file) = File::open(&server_properties_path).await {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).await.is_ok() {
                let motd_re = regex::Regex::new(r"(?m)^motd\s*=.*$").unwrap();
                let new_contents = if motd_re.is_match(&contents) {
                    motd_re.replace(&contents, format!("motd={}", motd_val)).to_string()
                } else {
                    format!("{}\nmotd={}", contents.trim_end(), motd_val)
                };
                if let Err(e) = fs::write(&server_properties_path, new_contents).await {
                    eprintln!("Failed to write server.properties: {:?}", e);
                    return Err((Status::InternalServerError, "Failed to write server.properties.".to_string()));
                }
            }
        }
    }

    let start_script = std::path::Path::new(&server_location).join("startserver.sh");
    let status = Command::new("chmod").arg("+x").arg(start_script).status();
    if !status.map_or(false, |s| s.success()) {
        eprintln!("Failed to chmod startserver.sh");
        return Err((Status::InternalServerError, "Failed to chmod startserver.sh.".to_string()));
    }

    let mods_list_dst = std::path::Path::new(&server_location).join("mods.list");
    if !mods_list_dst.exists() {
        let mods_dir = std::path::Path::new(&server_location).join("mods");
        if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
            let mut mod_names = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.ends_with(".jar") {
                            mod_names.push(name.to_string());
                        }
                    }
                }
            }
            let mods_list_content = mod_names.join("\n");
            if let Err(e) = fs::write(&mods_list_dst, mods_list_content).await {
                eprintln!("Failed to write mods.list: {:?}", e);
                return Err((Status::InternalServerError, "Failed to write mods.list.".to_string()));
            }
        }
    }

    Ok(Json(json!({"status": "Restore complete"})))
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

#[get("/check_server_update")]
async fn check_server_update() -> Json<serde_json::Value> {
    // 1. Read the local modpack version from $SERVER/config/bcc-common.toml
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let config_path = std::path::Path::new(&server_location).join("config/bcc-common.toml");
    let mut file = match File::open(&config_path).await {
        Ok(f) => f,
        Err(_) => return Json(json!({"error": "Could not open bcc-common.toml"})),
    };
    let mut contents = String::new();
    if file.read_to_string(&mut contents).await.is_err() {
        return Json(json!({"error": "Could not read bcc-common.toml"}));
    }
    let re = regex::Regex::new(r#"modpackVersion\s*=\s*\"([^\"]+)\""#).unwrap();
    let local_version = re.captures(&contents).and_then(|cap| cap.get(1)).map(|m| m.as_str().to_string());
    if local_version.is_none() {
        return Json(json!({"error": "Could not find modpackVersion in bcc-common.toml"}));
    }
    let local_version = local_version.unwrap();
    // 2. Fetch the latest modpack version from CurseForge API using the curseforge module
    let client = reqwest::Client::new();
    let latest = match fetch_latest_server_pack(&client).await {
        Ok(info) => info,
        Err(e) => return Json(json!({"error": e})),
    };
    // 3. Compare using semver if possible
    let up_to_date = match (Version::parse(&local_version), Version::parse(&latest.version)) {
        (Ok(local), Ok(latest)) => local == latest,
        _ => local_version == latest.version,
    };
    Json(json!({
        "local_version": local_version,
        "latest_version": latest.version,
        "up_to_date": up_to_date
    }))
}

#[post("/update_extras")]
async fn update_extras() -> Result<Status, (Status, String)> {
    if !systemctl_server(ServerAction::Stop) {
        return Err((Status::InternalServerError, "Failed to stop server.".to_string()));
    }

    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let mods_dir = std::path::Path::new(&server_location).join("mods");
    let extra_mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());

    let mods_list_path = std::path::Path::new(&server_location).join("mods.list");
    let allowed_mods: Vec<String> = match fs::read_to_string(&mods_list_path).await {
        Ok(contents) => contents.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Err(e) => {
            let err_msg = format!("Failed to read mods.list: {}", e);
            eprintln!("[update_extras] {}", err_msg);
            return Err((Status::InternalServerError, err_msg));
        }
    };

    if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") && !allowed_mods.contains(&name.to_string()) {
                        if let Err(e) = fs::remove_file(&path).await {
                            eprintln!("[update_extras] Failed to remove disallowed mod '{}': {:?}", name, e);
                        }
                    }
                }
            }
        }
    } else {
        let err_msg = format!("Failed to read mods directory: {}", mods_dir.display());
        eprintln!("[update_extras] {}", err_msg);
        return Err((Status::InternalServerError, err_msg));
    }

    if let Ok(mut entries) = fs::read_dir(&extra_mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") {
                        let dest = mods_dir.join(name);
                        if let Err(e) = fs::copy(&path, &dest).await {
                            eprintln!("[update_extras] Failed to copy extra mod '{}': {:?}", name, e);
                        }
                    }
                }
            }
        }
    }

    if !systemctl_server(ServerAction::Start) {
        return Err((Status::InternalServerError, "Failed to start server.".to_string()));
    }

    Ok(Status::Ok)
}

#[catch(400)]
fn bad_request(_req: &Request) -> &'static str {
    "400 Bad Request: The request was malformed or missing required data (e.g., file upload missing filename)."
}

#[launch]
fn rocket() -> rocket::Rocket<rocket::Build> {
    let mut config = Config::release_default();
    config.address = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
    config.limits = rocket::data::Limits::new()
        .limit("file", ByteUnit::Gibibyte(1)) // Increased file limit
        .limit("form", ByteUnit::Gibibyte(1)); // Increased form limit
    rocket::custom(config) 
        .mount("/", routes![
            index_html, 
            style_css, 
            start, 
            stop, 
            restart, 
            download_mods, 
            extra_mods_list, 
            delete_mod, 
            extra_mods_upload, 
            update_extras, 
            log_tail, 
            check_server_update, 
            backup_server, 
            restore_server
        ])
        .register("/", catchers![bad_request])
        .attach(static_resources_initializer!(
            "index-html" => ("src/page", "index.html"),
            "style-css" => ("src/page", "style.css"),
        ))
}
