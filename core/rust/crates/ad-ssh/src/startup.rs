//! Explicit startup preparation scoped to one newly opened connection.
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct StartupScript {
    pub commands: String,
    pub timeout_seconds: u64,
    pub continue_on_error: bool,
}
impl StartupScript {
    pub fn validate(&self) -> Result<(), String> {
        if self.commands.len() > 64 * 1024 || self.commands.contains('\0') {
            return Err(
                "Startup commands must be at most 64 KiB and contain no NUL characters".into(),
            );
        }
        if !(1..=600).contains(&self.timeout_seconds) {
            return Err("Startup timeout must be between 1 and 600 seconds".into());
        }
        Ok(())
    }
    pub fn environment_prefix(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!("AETHERDESK_STARTUP_COMMANDS={} AETHERDESK_STARTUP_TIMEOUT={} AETHERDESK_STARTUP_CONTINUE={} ", quote(&self.commands), self.timeout_seconds, u8::from(self.continue_on_error)))
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds.saturating_add(10))
    }
}
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
impl crate::SshSession {
    /// Environment exports and working directory are loaded into the terminal's
    /// actual interactive shell, rather than lost in a separate exec channel.
    pub async fn prepare_startup(
        &self,
        script: &StartupScript,
        shell: bool,
    ) -> Result<String, String> {
        let command = format!(
            "{}bash -c {} aetherdesk-startup {}",
            script.environment_prefix()?,
            quote(include_str!("session_startup.sh")),
            if shell { "prepare" } else { "exec /bin/true" }
        );
        let output = self
            .execute_command_output(&command, script.timeout())
            .await
            .map_err(|e| e.to_string())?;
        if output.exit_status != 0 {
            return Err(format!(
                "Startup commands failed (exit {}). {}",
                output.exit_status,
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(1200)
                    .collect::<String>()
            ));
        }
        if !shell {
            return Ok(String::new());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let path = text
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("AETHERDESK_STARTUP_ENV_V1="))
            .filter(|path| {
                path.starts_with('/')
                    && path.ends_with("/environment.sh")
                    && !path.chars().any(char::is_control)
            })
            .ok_or("Startup preparation did not return its private environment")?;
        let parent = path
            .rsplit_once('/')
            .ok_or("Invalid startup environment")?
            .0;
        Ok(format!(". {}; __ad_startup_status=$?; rm -f -- {}; rmdir -- {}; if [ \"$__ad_startup_status\" -ne 0 ]; then return \"$__ad_startup_status\"; fi; ", quote(path), quote(path), quote(parent)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn startup_exports_and_directory_survive_and_failure_timeout_policies_are_scoped() {
        use std::process::Command;
        let home = tempfile::tempdir().unwrap();
        let work = home.path().join("work with spaces");
        std::fs::create_dir(&work).unwrap();
        let run = |commands: &str, keep_going: bool, limit: u64| {
            Command::new("bash")
                .args([
                    "-c",
                    include_str!("session_startup.sh"),
                    "fixture",
                    "exec",
                    "bash",
                    "-c",
                    "printf '%s|%s' \"${AD_STARTUP_FIXTURE-unset}\" \"$PWD\"",
                ])
                .env("HOME", home.path())
                .env("AETHERDESK_STARTUP_COMMANDS", commands)
                .env("AETHERDESK_STARTUP_TIMEOUT", limit.to_string())
                .env(
                    "AETHERDESK_STARTUP_CONTINUE",
                    if keep_going { "1" } else { "0" },
                )
                .output()
                .unwrap()
        };
        let output = run(
            &format!(
                "export AD_STARTUP_FIXTURE=ready\ncd {}",
                quote(work.to_str().unwrap())
            ),
            false,
            5,
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("ready|{}", work.display())
        );
        assert_eq!(run("false", false, 5).status.code(), Some(1));
        assert!(run("false", true, 5).status.success());
        assert!(!run("exit 0", false, 5).status.success());
        let timeout = run("while :; do sleep 1; done", false, 1);
        assert_eq!(timeout.status.code(), Some(124));
        assert!(std::fs::read_dir(home.path().join(".aetherdesk/startup"))
            .unwrap()
            .next()
            .is_none());
    }
    #[test]
    fn startup_script_is_quoted_data_with_bounded_execution() {
        let script = StartupScript {
            commands: "export TOOL_HOME='/opt/tool';\ncd /work".into(),
            timeout_seconds: 30,
            continue_on_error: false,
        };
        assert!(script.environment_prefix().unwrap().contains("'\\''"));
        assert!(script
            .environment_prefix()
            .unwrap()
            .contains("STARTUP_CONTINUE=0"));
        assert!(StartupScript {
            timeout_seconds: 0,
            ..script.clone()
        }
        .validate()
        .is_err());
        assert!(StartupScript {
            commands: "\0".into(),
            ..script
        }
        .validate()
        .is_err());
    }
}
