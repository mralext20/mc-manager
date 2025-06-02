use std::process::Command;
use crate::constants::SYSTEMD_SERVICE;

#[derive(Debug, Clone, Copy)]
pub enum ServerAction {
    Start,
    Stop,
    Restart,
}

pub fn systemctl_server(action: ServerAction) -> bool {
    let action_str = match action {
        ServerAction::Start => "start",
        ServerAction::Stop => "stop",
        ServerAction::Restart => "restart",
    };
    Command::new("systemctl")
        .args(["--user", action_str, SYSTEMD_SERVICE])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
