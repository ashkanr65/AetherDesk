//! Windows commands stay behind an input gate until a kill-on-close job owns
//! the process. Cancellation drops the job and terminates its descendants.
#[cfg(windows)]
pub struct ProcessJob(isize);
#[cfg(windows)]
impl ProcessJob {
    pub fn attach(child: &tokio::process::Child) -> Result<Self, String> {
        use windows_sys::Win32::System::JobObjects::*;
        let process = child.raw_handle().ok_or("Command process already exited")?;
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err("Could not create the command cleanup job".into());
        }
        let job = Self(handle as isize);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(handle, process) } == 0
        {
            return Err(
                "Could not attach the command to its cleanup job; no command was executed".into(),
            );
        }
        Ok(job)
    }
}
#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn closing_a_job_terminates_only_its_owned_process_tree() {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        };
        struct ProcessHandle(HANDLE);
        impl Drop for ProcessHandle {
            fn drop(&mut self) {
                unsafe { CloseHandle(self.0) };
            }
        }
        let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        // Keep this separate process outside the dedicated job and behind an
        // unread input gate. Closing the owned job must not terminate it.
        let mut unrelated = tokio::process::Command::new(&powershell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$null=[Console]::ReadLine()",
            ])
            .creation_flags(0x08000000)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut command = tokio::process::Command::new(&powershell);
        // Spawn the descendant directly, so Windows job inheritance is tested
        // without routing process creation through shell associations.
        command.args(["-NoLogo","-NoProfile","-NonInteractive","-Command", "$null=[Console]::ReadLine(); $start=New-Object System.Diagnostics.ProcessStartInfo; $start.FileName=$PSHOME+'\\powershell.exe'; $start.Arguments='-NoLogo -NoProfile -NonInteractive -Command \"Start-Sleep -Seconds 60\"'; $start.UseShellExecute=$false; $start.CreateNoWindow=$true; $p=[System.Diagnostics.Process]::Start($start); [Console]::WriteLine($p.Id); Start-Sleep -Seconds 60"])
            .creation_flags(0x08000000).kill_on_drop(true).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        let mut child = command.spawn().unwrap();
        let job = ProcessJob::attach(&child).unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        let mut reader = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            reader.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let pid = line.trim().parse::<u32>().unwrap();
        let descendant = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!descendant.is_null());
        let descendant = ProcessHandle(descendant);
        assert!(child.try_wait().unwrap().is_none());
        assert!(unrelated.try_wait().unwrap().is_none());
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.0, 0) },
            WAIT_TIMEOUT
        );
        drop(job);
        // Kill-on-job-close can yield exit code zero. The contract is that
        // both live processes terminate, not a particular exit-code value.
        tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        let stopped = unsafe { WaitForSingleObject(descendant.0, 5000) };
        assert_eq!(stopped, WAIT_OBJECT_0);
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.start_kill().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), unrelated.wait())
            .await
            .unwrap()
            .unwrap();
    }
}
