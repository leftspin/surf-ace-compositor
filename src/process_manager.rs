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
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());

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
