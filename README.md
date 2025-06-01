# MC Manager

A web-based dashboard for managing a Minecraft server via systemd user services, built with Rust and Rocket.

## Features
- Start, stop, and restart a systemd user service (`atm10.service`) from a web UI
- Download a ZIP archive of extra mods, dynamically generated from a configurable directory
- Serves a static HTML dashboard (see `src/page/index.html`)

## Usage

### Prerequisites
- Rust (edition 2021 or later)
- systemd user service named `atm10.service`
- The `extra_mods` directory (or set the `EXTRA_MODS_DIR` environment variable)

### Running
```sh
# Optionally set the mods directory
export EXTRA_MODS_DIR=/path/to/your/extra_mods

# Build and run
cargo run
```

The web dashboard will be available at [http://localhost:8000](http://localhost:8000).

### Endpoints
- `/` — Dashboard UI
- `/start` — POST: Start the server
- `/stop` — POST: Stop the server
- `/restart` — POST: Restart the server
- `/mods.zip` — GET: Download all files in the mods directory as a ZIP

## Configuration
- `EXTRA_MODS_DIR`: Path to the directory containing extra mods to be zipped and downloaded. Defaults to `extra_mods` in the project root.

## Project Structure
```
mc-manager/
├── src/
│   ├── main.rs         # Rocket web server and handlers
│   └── page/
│       └── index.html  # Dashboard UI
├── extra_mods/         # (Default) Directory for extra mods
├── Cargo.toml
└── README.md
```

## Security Notes
- The server executes systemctl commands as the current user. Make sure only trusted users can access the web UI.
- The `/mods.zip` endpoint exposes all files in the configured mods directory.

## License
MIT
