use std::path::PathBuf;

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("Cannot determine app data directory")
        .join("PilotDesk")
}

pub fn db_path() -> PathBuf {
    app_data_dir().join("pilotdesk.db")
}

pub fn logs_dir() -> PathBuf {
    app_data_dir().join("logs")
}

pub fn claude_config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".claude")
}

pub fn hermes_config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".hermes")
}
