use crate::model::OutputRotation;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const OUTPUT_ROTATION_STATE_PATH_ENV: &str = "SURF_ACE_COMPOSITOR_OUTPUT_ROTATION_STATE_PATH";

#[derive(Debug, Clone)]
pub struct OutputRotationMemory {
    path: PathBuf,
}

impl OutputRotationMemory {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os(OUTPUT_ROTATION_STATE_PATH_ENV) {
            return Some(PathBuf::from(path));
        }
        if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
            return Some(
                PathBuf::from(state_home)
                    .join("surf-ace-compositor")
                    .join("output-rotation.json"),
            );
        }
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("surf-ace-compositor")
                .join("output-rotation.json")
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<Option<OutputRotation>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        serde_json::from_slice::<OutputRotation>(&bytes)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    pub fn store(&self, rotation: OutputRotation) -> io::Result<()> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let tmp_path = self
            .path
            .with_extension(format!("{}.{}.tmp", std::process::id(), unique));
        let bytes = serde_json::to_vec_pretty(&rotation).map_err(io::Error::other)?;
        fs::write(&tmp_path, bytes)?;
        fs::rename(tmp_path, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::OutputRotationMemory;
    use crate::model::OutputRotation;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        PathBuf::from(format!(
            "/tmp/surf-ace-output-rotation-memory-{}-{unique}.json",
            std::process::id()
        ))
    }

    #[test]
    fn output_rotation_memory_persists_and_restores_rotation() {
        let memory = OutputRotationMemory::new(temp_path());

        assert_eq!(memory.load().expect("missing state should load"), None);

        memory
            .store(OutputRotation::Deg90)
            .expect("rotation should store");

        assert_eq!(
            memory.load().expect("stored rotation should load"),
            Some(OutputRotation::Deg90)
        );
        let _ = std::fs::remove_file(memory.path());
    }
}
