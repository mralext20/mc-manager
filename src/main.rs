#[macro_use] extern crate rocket;
#[macro_use] extern crate rocket_include_static_resources;

use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::io::Write;
use rocket::tokio::fs::{self, File};
use rocket::tokio::io::AsyncReadExt;
use rocket::Config;
use zip::write::{FileOptions, ZipWriter, ExtendedFileOptions};
use rocket::response::content::RawJson;
use rocket::tokio::fs::remove_file;
use rocket::http::Status;
extern crate serde_json;



static_response_handler! {
    "/" => index_html => "index-html",
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

#[launch]
fn rocket() -> rocket::Rocket<rocket::Build> {
    let mut config = Config::release_default();
    config.address = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
    rocket::build()
        .attach(static_resources_initializer!(
            "index-html" => ("src/page", "index.html"),
        ))
        .mount("/", routes![index_html, start, stop, restart, download_mods, extra_mods_list, delete_mod, extra_mods_upload])
        .configure(config)
}
