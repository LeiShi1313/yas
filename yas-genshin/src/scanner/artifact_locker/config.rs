use std::path::PathBuf;

#[derive(Clone, clap::Args)]
pub struct GenshinArtifactLockerConfig {
    /// Apply lock changes from a yas-lock v1 or v2 JSON file
    #[arg(id = "lock-file", long = "lock-file", value_name = "PATH")]
    pub lock_file: Option<PathBuf>,

    /// Pause after clicking the lock button
    #[arg(
        id = "lock-stop",
        long = "lock-stop",
        value_name = "MILLISECONDS",
        default_value_t = 100
    )]
    pub lock_stop: u32,

    /// Maximum time to wait for a verified lock-state change
    #[arg(
        id = "max-wait-lock",
        long = "max-wait-lock",
        value_name = "MILLISECONDS",
        value_parser = clap::value_parser!(u32).range(1..),
        default_value_t = 800
    )]
    pub max_wait_lock: u32,
}
