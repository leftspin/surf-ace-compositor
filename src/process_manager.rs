use crate::model::ProcessSpec;
use std::collections::{BTreeMap, HashMap};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessExit {
    pub pid: u32,
    pub exit_code: Option<i32>,
}

pub trait ProcessController: Send {
    fn spawn(
        &mut self,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<u32, String>;
    fn terminate(&mut self, pid: u32) -> Result<(), String>;
    fn reap_exited(&mut self) -> Vec<ProcessExit>;
}

#[derive(Default)]
pub struct LocalProcessController {
    children: HashMap<u32, Child>,
}

impl ProcessController for LocalProcessController {
    fn spawn(
        &mut self,
        spec: &ProcessSpec,
        extra_env: &BTreeMap<String, String>,
    ) -> Result<u32, String> {
        let mut command = Command::new(&spec.command);
        command.args(&spec.args);
        command.stdin(Stdio::null());

        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }

        for (key, value) in &spec.env {
            command.env(key, value);
        }
        for (key, value) in extra_env {
            command.env(key, value);
        }

        let child = command
            .spawn()
            .map_err(|err| format!("failed to spawn process '{}': {err}", spec.command))?;
        let pid = child.id();
        self.children.insert(pid, child);
        Ok(pid)
    }

    fn terminate(&mut self, pid: u32) -> Result<(), String> {
        let Some(mut child) = self.children.remove(&pid) else {
            return Err(format!("unknown pid: {pid}"));
        };
        child
            .kill()
            .map_err(|err| format!("failed to terminate pid {pid}: {err}"))?;
        let _ = child.wait();
        Ok(())
    }

    fn reap_exited(&mut self) -> Vec<ProcessExit> {
        let mut exited = Vec::new();
        let mut to_remove = Vec::new();

        for (pid, child) in &mut self.children {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited.push(ProcessExit {
                        pid: *pid,
                        exit_code: status.code(),
                    });
                    to_remove.push(*pid);
                }
                Ok(None) => {}
                Err(_) => {
                    exited.push(ProcessExit {
                        pid: *pid,
                        exit_code: None,
                    });
                    to_remove.push(*pid);
                }
            }
        }

        for pid in to_remove {
            self.children.remove(&pid);
        }

        exited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn local_process_controller_preserves_child_stdout_and_stderr_diagnostics() {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let output_path = std::env::temp_dir().join(format!(
            "surf-ace-process-fds-{}-{started_at}.txt",
            std::process::id()
        ));
        let script = format!(
            "readlink /proc/$$/fd/1 > {path}; readlink /proc/$$/fd/2 >> {path}",
            path = shell_quote(output_path.to_string_lossy().as_ref())
        );
        let process = ProcessSpec {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
            cwd: None,
            env: BTreeMap::new(),
        };
        let mut controller = LocalProcessController::default();

        let pid = controller
            .spawn(&process, &BTreeMap::new())
            .expect("diagnostic child should spawn");

        let mut contents = String::new();
        for _ in 0..50 {
            controller.reap_exited();
            if let Ok(value) = fs::read_to_string(&output_path) {
                contents = value;
                if contents.lines().count() >= 2 {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        let _ = fs::remove_file(&output_path);
        let fds: Vec<&str> = contents.lines().collect();
        assert_eq!(
            fds.len(),
            2,
            "child {pid} should record stdout and stderr fds: {contents:?}"
        );
        assert_ne!(
            fds[0], "/dev/null",
            "child stdout must not discard diagnostics"
        );
        assert_ne!(
            fds[1], "/dev/null",
            "child stderr must not discard diagnostics"
        );
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
