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
async fn download_mods() -> Option<(rocket::http::ContentType, Vec<u8>)> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
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
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
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
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
    let path = std::path::Path::new(&mods_dir).join(modname);
    match remove_file(&path).await {
        Ok(_) => "OK",
        Err(_) => "FAIL",
    }
}

#[post("/extra_mods_upload", data = "<mod>")]
async fn extra_mods_upload(mut r#mod: rocket::fs::TempFile<'_>) -> Result<Status, Status> {
    let mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());

    // Try to get the filename from the uploaded file, fallback to Content-Disposition if needed
    let filename = match r#mod.raw_name() {
        Some(raw_name) => {
            let fname_str = raw_name.dangerous_unsafe_unsanitized_raw().as_str();
            if fname_str.is_empty() {
                eprintln!("Uploaded file has an empty filename.");
                return Err(Status::BadRequest);
            }
            println!("Received raw filename for upload: {}", fname_str);
            fname_str.to_string()
        }
        None => {
            // Try to get the name from the form field (for some clients)
            if let Some(name) = r#mod.name() {
                if !name.is_empty() && name.ends_with(".jar") {
                    println!("Fallback: using form field name as filename: {}", name);
                    name.to_string()
                } else {
                    eprintln!("Uploaded file is missing a filename.");
                    return Err(Status::BadRequest);
                }
            } else {
                eprintln!("Uploaded file is missing a filename.");
                return Err(Status::BadRequest);
            }
        }
    };

    if !filename.ends_with(".jar") {
        eprintln!("Invalid file type or filename: '{}'. Must be a .jar file.", filename);
        return Err(Status::BadRequest);
    }

    println!("Processing upload for mod: {}", filename);

    // Ensure the target directory exists
    if let Err(e) = fs::create_dir_all(&mods_dir).await {
        eprintln!("Failed to create mods directory '{}': {:?}", mods_dir, e);
        return Err(Status::InternalServerError);
    }

    let dest_path = std::path::Path::new(&mods_dir).join(&filename);

    match r#mod.persist_to(&dest_path).await {
        Ok(_) => {
            println!("Successfully saved mod to: {}", dest_path.display());
            Ok(Status::Ok)
        }
        Err(e) => {
            eprintln!("Failed to persist uploaded file '{}' to '{}': {:?}", filename, dest_path.display(), e);
            Err(Status::InternalServerError)
        }
    }
}

#[post("/backup_server")]
async fn backup_server() -> Json<serde_json::Value> {
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let backup_dir = format!("{}_backup", server_location);
    let files_to_backup = FILES_TO_BACKUP;
    let _ = rocket::tokio::fs::remove_dir_all(&backup_dir).await;
    let _ = rocket::tokio::fs::create_dir_all(&backup_dir).await;
    // Write mods.list before backup
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
        let _ = fs::write(&mods_list_path, mods_list_content).await;
    }
    for item in files_to_backup.iter() {
        let src = std::path::Path::new(&server_location).join(item);
        let dst = std::path::Path::new(&backup_dir).join(item);
        if src.exists() {
            if src.is_dir() {
                let _ = std::process::Command::new("cp").args(["-r", src.to_str().unwrap(), dst.to_str().unwrap()]).status();
            } else {
                let _ = rocket::tokio::fs::copy(&src, &dst).await;
            }
        }
    }
    Json(json!({"status": "Backup complete"}))
}

#[post("/restore_server")]
async fn restore_server() -> Json<serde_json::Value> {
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let backup_dir = format!("{}_backup", server_location);
    let files_to_backup = FILES_TO_BACKUP;
    for item in files_to_backup.iter() {
        let src = std::path::Path::new(&backup_dir).join(item);
        let dst = std::path::Path::new(&server_location).join(item);
        if src.exists() {
            if src.is_dir() {
                let _ = std::process::Command::new("cp").args(["-r", src.to_str().unwrap(), dst.to_str().unwrap()]).status();
            } else {
                let _ = rocket::tokio::fs::copy(&src, &dst).await;
            }
        }
    }
    // Patch server.properties MOTD and chmod start script after restore
    let config_path = std::path::Path::new(&server_location).join("config/bcc-common.toml");
    // Extract version from bcc-common.toml
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
                let _ = rocket::tokio::fs::write(&server_properties_path, new_contents).await;
            }
        }
    }
    // chmod +x startserver.sh
    let start_script = std::path::Path::new(&server_location).join("startserver.sh");
    let _ = std::process::Command::new("chmod").arg("+x").arg(start_script).status();
    // On restore: if mods.list does not exist, generate it from current mods directory
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
            let _ = fs::write(&mods_list_dst, mods_list_content).await;
        }
    }
    Json(json!({"status": "Restore complete"}))
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
async fn update_extras() -> Status {
    // 1. Stop the server
    if !systemctl_server(ServerAction::Stop) {
        return Status::InternalServerError;
    }
    // 2. Determine server location
    let server_location = std::env::var("SERVER_LOCATION").unwrap_or_else(|_| DEFAULT_SERVER_LOCATION.to_string());
    let mods_dir = std::path::Path::new(&server_location).join("mods");
    let extra_mods_dir = std::env::var("EXTRA_MODS_DIR").unwrap_or_else(|_| DEFAULT_EXTRA_MODS_DIR.to_string());
    // 3. Read allowed mods from mods.list in the server directory
    let mods_list_path = std::path::Path::new(&server_location).join("mods.list");
    let allowed_mods: Vec<String> = match fs::read_to_string(&mods_list_path).await {
        Ok(contents) => contents.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Err(e) => {
            println!("[update_extras] Failed to read mods.list at {}: {}", mods_list_path.display(), e);
            panic!("Failed to read mods.list: {}", e);
        }
    };
    println!("[update_extras] Allowed mods from mods.list: {:?}", allowed_mods);
    // 4. Delete .jar files in mods/ that are not in allowed_mods
    if let Ok(mut entries) = fs::read_dir(&mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".jar") && !allowed_mods.contains(&name.to_string()) {
                        println!("[update_extras] Removing disallowed mod: {}", name);
                        let _ = fs::remove_file(&path).await;
                    }
                }
            }
        }
    } else {
        println!("[update_extras] Failed to read mods directory: {}", mods_dir.display());
        panic!("Failed to read mods directory: {}", mods_dir.display());
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
    if !systemctl_server(ServerAction::Start) {
        return Status::InternalServerError;
    }
    Status::Ok
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
