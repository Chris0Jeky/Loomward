use super::{capability_names, command_line, Role};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io,
    mem::{size_of, zeroed},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    os::windows::{ffi::OsStrExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::{null, null_mut},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        Isolation::{DeriveAppContainerSidFromAppContainerName, GetAppContainerFolderPath},
        *,
    },
    System::{Com::CoTaskMemFree, JobObjects::*, SystemServices::SE_GROUP_ENABLED, Threading::*},
    UI::Shell::{SHGetFolderPathW, CSIDL_PERSONAL, SHGFP_TYPE_CURRENT},
};

const PINNED_CODEX: &str = "9e7c59c05cc1ce5677b1f94e835b2ac038ca3be14504e78d558eacdb0ea3f55d";
const SYNTHETIC_PROMPT: &str = "Reply with the fixed synthetic string LOOMWARD_SYNTHETIC_ONLY.";

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}
fn checked(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn outcome<T>(result: io::Result<T>) -> Value {
    match result {
        Ok(_) => json!({"ok": true, "error_code": null}),
        Err(e) => {
            json!({"ok": false, "error_code": e.raw_os_error(), "kind": format!("{:?}", e.kind())})
        }
    }
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Sid(PSID);
impl Drop for Sid {
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.0);
        }
    }
}
fn sid_text(sid: PSID) -> io::Result<String> {
    unsafe {
        let mut text = null_mut();
        checked(ConvertSidToStringSidW(sid, &mut text))?;
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(value)
    }
}
fn token() -> io::Result<Handle> {
    let mut handle = null_mut();
    unsafe {
        checked(OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut handle,
        ))?;
    }
    Ok(Handle(handle))
}
fn token_value<T: Default>(class: TOKEN_INFORMATION_CLASS) -> io::Result<T> {
    let token = token()?;
    let mut value = T::default();
    let mut size = 0;
    unsafe {
        checked(GetTokenInformation(
            token.0,
            class,
            (&mut value as *mut T).cast(),
            size_of::<T>() as u32,
            &mut size,
        ))?;
    }
    Ok(value)
}
fn token_buffer(class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<usize>> {
    let token = token()?;
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.0, class, null_mut(), 0, &mut size);
    }
    let mut buf = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    unsafe {
        checked(GetTokenInformation(
            token.0,
            class,
            buf.as_mut_ptr().cast(),
            size,
            &mut size,
        ))?;
    }
    Ok(buf)
}
fn identity() -> io::Result<Value> {
    let contained = token_value::<u32>(TokenIsAppContainer)? != 0;
    let sid = if contained {
        let buf = token_buffer(TokenAppContainerSid)?;
        let info = unsafe { &*buf.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>() };
        Some(sid_text(info.TokenAppContainer)?)
    } else {
        None
    };
    let mut in_job = 0;
    unsafe {
        checked(IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job))?;
    }
    Ok(json!({"appcontainer": contained, "container_sid": sid, "in_job": in_job != 0}))
}

// Both ACL operations address only directories created by this invocation.
fn directory_acl(path: &Path, owner: &str, container: Option<&str>) -> io::Result<()> {
    let grant = container
        .map(|sid| format!("(A;OICI;FA;;;{sid})"))
        .unwrap_or_default();
    let sddl = format!("D:P(A;OICI;FA;;;{owner})(A;OICI;FA;;;SY){grant}S:(ML;OICI;NW;;;LW)");
    unsafe {
        let mut descriptor = null_mut();
        checked(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(&sddl).as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        ))?;
        let result = checked(SetFileSecurityW(
            wide(path).as_ptr(),
            DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION
                | LABEL_SECURITY_INFORMATION,
            descriptor,
        ));
        LocalFree(descriptor);
        result
    }
}

#[derive(Serialize, Deserialize)]
struct ProbeConfig {
    canary: PathBuf,
    allowed: PathBuf,
    documents: PathBuf,
    endpoints: Vec<SocketAddr>,
    report: PathBuf,
    job: bool,
}
fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)
}
fn read_json(path: &Path) -> io::Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn public_log(path: &Path, root: &Path) -> String {
    let mut text = fs::read_to_string(path).unwrap_or_default();
    for (path, replacement) in [
        (root.to_string_lossy().into_owned(), "<lab-root>"),
        (
            std::env::var("USERPROFILE").unwrap_or_default(),
            "<user-profile>",
        ),
        (std::env::var("APPDATA").unwrap_or_default(), "<app-data>"),
        (
            std::env::var("SystemRoot").unwrap_or_default(),
            "<system-root>",
        ),
    ] {
        if !path.is_empty() {
            text = text
                .replace(&path, replacement)
                .replace(&path.replace('\\', "/"), replacement)
                .replace(&path.replace('\\', "\\\\"), replacement);
        }
    }
    text
}

fn probe(config: &Path) -> io::Result<()> {
    let cfg: ProbeConfig = serde_json::from_slice(&fs::read(config)?)?;
    let parent = identity()?;
    if parent["appcontainer"] != true {
        return Err(io::Error::other("probe refuses an uncontained token"));
    }
    let files = json!({
        "ungranted_canary": outcome(fs::read(&cfg.canary)),
        "granted_canary": outcome(fs::read(&cfg.allowed)),
        // Opening the iterator tests listing permission; never retain names or contents.
        "documents_listing": outcome(fs::read_dir(&cfg.documents)),
    });
    let network: Vec<_> = cfg.endpoints.iter().map(|addr| {
        json!({"endpoint": addr.to_string(), "result": outcome(TcpStream::connect_timeout(addr, Duration::from_secs(4)))})
    }).collect();
    let child_report = cfg.report.with_extension("child.json");
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--child")
        .arg(&child_report)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if cfg.job {
        command.arg("--linger");
    }
    let child = match command.spawn() {
        Ok(mut child) => {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !child_report.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            let measured = read_json(&child_report).ok();
            if !cfg.job {
                child.wait()?;
            }
            json!({"spawn": {"ok": true, "error_code": null}, "pid": child.id(), "identity": measured})
        }
        Err(e) => json!({"spawn": outcome::<()>(Err(e))}),
    };
    let breakaway_report = cfg.report.with_extension("breakaway.json");
    let breakaway = if cfg.job {
        match Command::new(std::env::current_exe()?)
            .arg("--child")
            .arg(&breakaway_report)
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                child.wait()?;
                json!({"ok": true, "error_code": null, "identity": read_json(&breakaway_report).ok()})
            }
            Err(e) => outcome::<()>(Err(e)),
        }
    } else {
        json!({"not_run": "no explicit job in this case"})
    };
    let mut ui: JOBOBJECT_BASIC_UI_RESTRICTIONS = unsafe { zeroed() };
    let ui_query = if cfg.job {
        unsafe {
            outcome(checked(QueryInformationJobObject(
                null_mut(),
                JobObjectBasicUIRestrictions,
                (&mut ui as *mut JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
                size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
                null_mut(),
            )))
        }
    } else {
        Value::Null
    };
    write_json(
        &cfg.report,
        &json!({"identity": parent, "filesystem": files, "network": network, "child": child, "breakaway": breakaway, "ui_restrictions_query": ui_query, "ui_restrictions": ui.UIRestrictionsClass}),
    )
}

struct Attributes {
    storage: Vec<usize>,
    initialized: bool,
}
impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes);
        }
        let mut value = Self {
            storage: vec![0; bytes.div_ceil(size_of::<usize>())],
            initialized: false,
        };
        unsafe {
            checked(InitializeProcThreadAttributeList(
                value.ptr(),
                count,
                0,
                &mut bytes,
            ))?;
        }
        value.initialized = true;
        Ok(value)
    }
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    fn set<T>(&mut self, attribute: u32, value: &mut T) -> io::Result<()> {
        unsafe {
            checked(UpdateProcThreadAttribute(
                self.ptr(),
                0,
                attribute as usize,
                (value as *mut T).cast(),
                size_of::<T>(),
                null_mut(),
                null(),
            ))
        }
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.ptr());
            }
        }
    }
}
fn security_capabilities(
    container: PSID,
    capability: Option<&mut SID_AND_ATTRIBUTES>,
) -> SECURITY_CAPABILITIES {
    let count = u32::from(capability.is_some());
    SECURITY_CAPABILITIES {
        AppContainerSid: container,
        Capabilities: capability.map(|c| c as *mut _).unwrap_or(null_mut()),
        CapabilityCount: count,
        Reserved: 0,
    }
}
fn job() -> io::Result<Handle> {
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = Handle(handle);
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: 255,
    };
    unsafe {
        checked(SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ))?;
        checked(SetInformationJobObject(
            job.0,
            JobObjectBasicUIRestrictions,
            (&ui as *const JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
            size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        ))?;
    }
    Ok(job)
}

struct Launch<'a> {
    exe: &'a Path,
    args: Vec<String>,
    lab: &'a Path,
    tag: &'a str,
    container: PSID,
    internet: bool,
    job: Option<&'a Handle>,
    stdin: &'a str,
}
fn launch(spec: Launch<'_>) -> io::Result<Value> {
    let input_path = spec.lab.join(format!("{}.stdin", spec.tag));
    fs::write(&input_path, spec.stdin)?;
    let input = File::open(&input_path)?;
    let output = File::create(spec.lab.join(format!("{}.stdout", spec.tag)))?;
    let error = File::create(spec.lab.join(format!("{}.stderr", spec.tag)))?;
    let mut handles = [
        input.as_raw_handle(),
        output.as_raw_handle(),
        error.as_raw_handle(),
    ];
    for handle in &handles {
        unsafe {
            checked(SetHandleInformation(
                *handle,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            ))?;
        }
    }
    let mut capability_sid = [0usize; 9];
    let mut sid_bytes = size_of_val(&capability_sid) as u32;
    if spec.internet {
        unsafe {
            checked(CreateWellKnownSid(
                WinCapabilityInternetClientSid,
                null_mut(),
                capability_sid.as_mut_ptr().cast(),
                &mut sid_bytes,
            ))?;
        }
    }
    let mut capability = SID_AND_ATTRIBUTES {
        Sid: capability_sid.as_mut_ptr().cast(),
        Attributes: SE_GROUP_ENABLED as u32,
    };
    let mut security = security_capabilities(
        spec.container,
        if spec.internet {
            Some(&mut capability)
        } else {
            None
        },
    );
    let mut attrs =
        Attributes::new(1 + u32::from(!spec.container.is_null()) + u32::from(spec.job.is_some()))?;
    if !spec.container.is_null() {
        attrs.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &mut security)?;
    }
    attrs.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &mut handles)?;
    let mut jobs = [spec.job.map(|j| j.0).unwrap_or(null_mut())];
    if spec.job.is_some() {
        attrs.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &mut jobs)?;
    }
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attrs.ptr();
    let mut argv = vec![spec.exe.to_string_lossy().into_owned()];
    argv.extend(spec.args);
    let mut cmd = wide(command_line(&argv));
    let system_root =
        std::env::var_os("SystemRoot").ok_or_else(|| io::Error::other("SystemRoot missing"))?;
    let mut env = Vec::new();
    for (key, value) in [
        ("CODEX_HOME", spec.lab.join("home")),
        ("SystemRoot", PathBuf::from(system_root)),
        ("TEMP", spec.lab.into()),
        ("TMP", spec.lab.into()),
    ] {
        env.extend(wide(format!("{key}={}", value.display())));
    }
    env.push(0);
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            wide(spec.exe).as_ptr(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            env.as_ptr().cast(),
            wide(spec.lab).as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    };
    if created == 0 {
        return Ok(json!({"create_process": outcome::<()>(Err(io::Error::last_os_error()))}));
    }
    let process_handle = Handle(process.hProcess);
    let _thread_handle = Handle(process.hThread);
    let wait = unsafe { WaitForSingleObject(process_handle.0, 20_000) };
    let timed_out = wait == WAIT_TIMEOUT;
    if timed_out {
        unsafe {
            checked(TerminateProcess(process_handle.0, 124))?;
            checked((WaitForSingleObject(process_handle.0, 5_000) == WAIT_OBJECT_0) as i32)?;
        }
    } else if wait != WAIT_OBJECT_0 {
        return Err(io::Error::last_os_error());
    }
    let mut exit = 0;
    unsafe {
        checked(GetExitCodeProcess(process_handle.0, &mut exit))?;
    }
    Ok(
        json!({"create_process": {"ok": true, "error_code": null}, "exit_code": exit, "timed_out": timed_out}),
    )
}

fn documents() -> io::Result<PathBuf> {
    let mut buf = [0u16; 260];
    let hr = unsafe {
        SHGetFolderPathW(
            null_mut(),
            CSIDL_PERSONAL as i32,
            null_mut(),
            SHGFP_TYPE_CURRENT as u32,
            buf.as_mut_ptr(),
        )
    };
    if hr < 0 {
        return Err(io::Error::other(format!(
            "Documents resolution HRESULT {hr:#x}"
        )));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())],
    )))
}

fn measure(root: &Path) -> io::Result<Value> {
    let buf = token_buffer(TokenUser)?;
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    let owner = sid_text(user.User.Sid)?;
    directory_acl(root, &owner, None)?;
    let name = format!(
        "Loomward.TeacherSpike.{}",
        root.file_name().unwrap().to_string_lossy()
    );
    let mut sid = null_mut();
    let hr = unsafe { DeriveAppContainerSidFromAppContainerName(wide(&name).as_ptr(), &mut sid) };
    if hr < 0 {
        return Err(io::Error::other(format!("derive SID HRESULT {hr:#x}")));
    }
    let sid = Sid(sid);
    let container = sid_text(sid.0)?;
    let mut folder = null_mut();
    let folder_hr = unsafe { GetAppContainerFolderPath(wide(&container).as_ptr(), &mut folder) };
    let folder_exists = if folder_hr >= 0 && !folder.is_null() {
        let mut len = 0;
        unsafe {
            while *folder.add(len) != 0 {
                len += 1;
            }
        }
        let path = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(folder, len)) };
        Some(Path::new(&path).exists())
    } else {
        None
    };
    unsafe {
        CoTaskMemFree(folder.cast());
    }
    let profile_lookup = json!({"api": "GetAppContainerFolderPath", "hresult": format!("0x{:08x}", folder_hr as u32), "folder_exists": folder_exists});
    let denied = root.join("ungranted");
    let lab = root.join("granted");
    fs::create_dir(&denied)?;
    fs::create_dir(&lab)?;
    directory_acl(&denied, &owner, None)?;
    directory_acl(&lab, &owner, Some(&container))?;
    let canary = denied.join("synthetic.txt");
    let allowed = lab.join("synthetic.txt");
    fs::write(&canary, "LOOMWARD_SYNTHETIC_CANARY")?;
    fs::write(&allowed, "LOOMWARD_SYNTHETIC_CANARY")?;
    fs::create_dir(lab.join("home"))?;
    let exe = lab.join("teacher_sandbox.exe");
    fs::copy(std::env::current_exe()?, &exe)?;
    let documents = documents()?;
    let api = ("api.openai.com", 443)
        .to_socket_addrs()?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| io::Error::other("no IPv4 API address"))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoints = vec!["1.1.1.1:443".parse().unwrap(), api, listener.local_addr()?];
    let controls: Vec<_> = endpoints.iter().map(|addr| json!({"endpoint": addr.to_string(), "result": outcome(TcpStream::connect_timeout(addr, Duration::from_secs(4)))})).collect();
    let launch_control = launch(Launch {
        exe: &exe,
        args: vec!["--child".into(), "-".into()],
        lab: &lab,
        tag: "control",
        container: null_mut(),
        internet: false,
        job: None,
        stdin: "",
    })?;
    let control_identity = read_json(&lab.join("control.stdout")).unwrap_or(Value::Null);
    let control_stderr = public_log(&lab.join("control.stderr"), root);
    if launch_control["create_process"]["ok"] != true
        || launch_control["exit_code"] != 0
        || control_identity["appcontainer"] != false
    {
        return Err(io::Error::other("ordinary launch control failed"));
    }
    let mut cases = Vec::new();
    let mut launch_blocked = false;
    for (internet, with_job) in [(false, false), (true, false), (false, true), (true, true)] {
        if launch_blocked {
            cases.push(json!({"capabilities": capability_names(internet), "job": with_job, "not_run": "stopped after first AppContainer launch failure"}));
            continue;
        }
        let tag = format!(
            "{}-{}",
            if internet { "internet" } else { "none" },
            if with_job { "job" } else { "plain" }
        );
        let report = lab.join(format!("{tag}.json"));
        let config = lab.join(format!("{tag}.config.json"));
        write_json(
            &config,
            &ProbeConfig {
                canary: canary.clone(),
                allowed: allowed.clone(),
                documents: documents.clone(),
                endpoints: endpoints.clone(),
                report: report.clone(),
                job: with_job,
            },
        )?;
        let job = if with_job { Some(job()?) } else { None };
        let launched = launch(Launch {
            exe: &exe,
            args: vec!["--probe".into(), config.to_string_lossy().into_owned()],
            lab: &lab,
            tag: &tag,
            container: sid.0,
            internet,
            job: job.as_ref(),
            stdin: "",
        });
        let launch_result = match launched {
            Ok(value) => value,
            Err(e) => json!({"setup": outcome::<()>(Err(e))}),
        };
        let measured = read_json(&report).ok();
        let child_handle = measured
            .as_ref()
            .and_then(|r| r["child"]["pid"].as_u64())
            .and_then(|pid| {
                let handle = unsafe {
                    OpenProcess(
                        PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        pid as u32,
                    )
                };
                if handle.is_null() {
                    None
                } else {
                    Some(Handle(handle))
                }
            });
        let alive_before = child_handle
            .as_ref()
            .map(|h| unsafe { WaitForSingleObject(h.0, 0) == WAIT_TIMEOUT });
        drop(job);
        let killed_on_close = if with_job {
            child_handle
                .as_ref()
                .map(|h| unsafe { WaitForSingleObject(h.0, 5_000) == WAIT_OBJECT_0 })
        } else {
            None
        };
        cases.push(json!({"capabilities": capability_names(internet), "job": with_job, "launch": launch_result, "probe": measured, "child_alive_before_job_close": alive_before, "child_exited_after_job_close": killed_on_close}));
        // A failed first launch is a stop, never an invitation to register a profile.
        launch_blocked = cases.last().unwrap()["launch"]["create_process"]["ok"] != true;
    }
    let source = PathBuf::from(std::env::var_os("APPDATA").ok_or_else(|| io::Error::other("APPDATA missing"))?).join("npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
    let codex = match fs::read(&source) {
        Ok(binary) => {
            let digest = format!("{:x}", Sha256::digest(&binary));
            if digest != PINNED_CODEX {
                json!({"blocked": "pinned_binary_mismatch", "sha256": digest})
            } else if cases.first().unwrap()["launch"]["create_process"]["ok"] != true {
                json!({"blocked": "container_launch_failed", "sha256": digest})
            } else {
                let native = lab.join("codex.exe");
                fs::write(&native, binary)?;
                let job = job()?;
                let mut results = Vec::new();
                for (tag, args, stdin) in [
                    ("codex-version", vec!["--version".into()], ""),
                    (
                        "codex-exec",
                        vec![
                            "--no-daemon".into(),
                            "exec".into(),
                            "--ignore-user-config".into(),
                            "--ignore-rules".into(),
                            "--ephemeral".into(),
                            "--skip-git-repo-check".into(),
                            "-s".into(),
                            "read-only".into(),
                            "-m".into(),
                            "gpt-6.1-sol".into(),
                            "-c".into(),
                            "model_reasoning_effort=\"medium\"".into(),
                            "-c".into(),
                            "approval_policy=\"never\"".into(),
                            "--disable".into(),
                            "code_mode_host".into(),
                            "--disable".into(),
                            "shell_tool".into(),
                            "--disable".into(),
                            "multi_agent".into(),
                            "--json".into(),
                            "-".into(),
                        ],
                        SYNTHETIC_PROMPT,
                    ),
                ] {
                    let result = match launch(Launch {
                        exe: &native,
                        args: args.clone(),
                        lab: &lab,
                        tag,
                        container: sid.0,
                        internet: false,
                        job: Some(&job),
                        stdin,
                    }) {
                        Ok(v) => v,
                        Err(e) => json!({"setup": outcome::<()>(Err(e))}),
                    };
                    let clean = |suffix| public_log(&lab.join(format!("{tag}.{suffix}")), root);
                    results.push(json!({"role": tag, "argv": args, "launch": result, "stdout": clean("stdout"), "stderr": clean("stderr")}));
                }
                json!({"sha256": digest, "capabilities": [], "auth": "fresh empty CODEX_HOME; owner auth never copied or granted", "runs": results, "tool_canaries": "blocked-by-design: no owner authentication; no authenticated inference attempted"})
            }
        }
        Err(e) => json!({"discovery": outcome::<()>(Err(e))}),
    };
    Ok(
        json!({"schema": 1, "platform": "Windows", "measured_at_unix_seconds": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(), "elevated": false, "profile_registered": false, "profile_lookup": profile_lookup, "container_sid": container, "controls": {"process_launch": launch_control, "identity": control_identity, "stderr": control_stderr, "ungranted_canary": outcome(fs::read(canary)), "granted_canary": outcome(fs::read(allowed)), "documents_listing": outcome(fs::read_dir(documents)), "network": controls}, "cases": cases, "codex": codex}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capability_builder_sets_only_the_explicit_sid() {
        let empty = security_capabilities(null_mut(), None);
        assert_eq!(empty.CapabilityCount, 0);
        assert!(empty.Capabilities.is_null());
        assert_eq!(empty.Reserved, 0);
        let mut buf = [0usize; 9];
        let mut bytes = size_of_val(&buf) as u32;
        unsafe {
            checked(CreateWellKnownSid(
                WinCapabilityInternetClientSid,
                null_mut(),
                buf.as_mut_ptr().cast(),
                &mut bytes,
            ))
            .unwrap();
        }
        let mut capability = SID_AND_ATTRIBUTES {
            Sid: buf.as_mut_ptr().cast(),
            Attributes: SE_GROUP_ENABLED as u32,
        };
        assert_eq!(sid_text(capability.Sid).unwrap(), "S-1-15-3-1");
        let pointer = &mut capability as *mut _;
        let internet = security_capabilities(null_mut(), Some(&mut capability));
        assert_eq!(internet.CapabilityCount, 1);
        assert_eq!(internet.Capabilities, pointer);
        assert_eq!(internet.Reserved, 0);
    }
}

pub(super) fn run(role: Role) -> io::Result<()> {
    if token_value::<TOKEN_ELEVATION>(TokenElevation)?.TokenIsElevated != 0 {
        return Err(io::Error::other("refusing elevated token"));
    }
    match role {
        Role::Probe(config) => probe(&config),
        Role::Child(path, linger) => {
            let who = identity()?;
            if path == Path::new("-") {
                println!("{who}");
            } else {
                write_json(&path, &who)?;
            }
            if linger {
                thread::sleep(Duration::from_secs(60));
            }
            Ok(())
        }
        Role::Run(output) => {
            let profile = std::env::var_os("USERPROFILE")
                .ok_or_else(|| io::Error::other("USERPROFILE missing"))?;
            let root = tempfile::Builder::new()
                .prefix("loomward-teacher-sandbox-")
                .tempdir_in(profile)?;
            let result = measure(root.path());
            let removed_path = root.path().to_path_buf();
            let cleanup = root.close();
            let mut receipt = match result {
                Ok(value) => value,
                Err(e) => json!({"measurement_error": outcome::<()>(Err(e))}),
            };
            receipt["cleanup"] = outcome(cleanup);
            receipt["cleanup"]["root_removed"] = json!(!removed_path.exists());
            write_json(&output, &receipt)?;
            if receipt.get("measurement_error").is_some() || receipt["cleanup"]["ok"] != true {
                return Err(io::Error::other(
                    "measurement or cleanup failed; see receipt",
                ));
            }
            Ok(())
        }
    }
}
