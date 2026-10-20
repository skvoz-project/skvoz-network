//! Host-only metadata proof for this fixture; never enters an actor or runs commands.
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::Path,
};

const ROOT: &str = "/sys/fs/cgroup";
const GIB: u64 = 1024 * 1024 * 1024;

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let mut value = String::new();
    fs::File::open(path)
        .unwrap()
        .take(1024 * 1024 + 1)
        .read_to_string(&mut value)
        .unwrap();
    assert!(value.len() <= 1024 * 1024, "host metadata bound exceeded");
    value
}
fn json_file(path: impl AsRef<Path>) -> Value {
    skvoz_network::local_api::parse_strict_json_bounded(read(path).as_bytes(), 1024 * 1024).unwrap()
}
fn number(value: &str) -> u64 {
    value.trim().parse().unwrap()
}
fn cpus(value: &str) -> BTreeSet<u64> {
    let mut result = BTreeSet::new();
    for part in value.trim().split(',') {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let (first, last) = (number(first), number(last));
        assert!(first <= last && last < 65536);
        result.extend(first..=last);
    }
    result
}
fn profile(role: &str) -> (u64, u64, u64, u64, u64) {
    match role {
        "prepare" => (400000, 3 * GIB, 128, 8388608, 2097152),
        "endpoint" => (100000, GIB / 2, 64, 2097152, 524288),
        "server" | "client" => (150000, GIB, 96, 3145728, 786432),
        _ => panic!("unknown host proof role"),
    }
}
fn mount() {
    let mounts = read("/proc/self/mountinfo");
    let matches: Vec<_> = mounts
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ").unwrap();
            let fields: Vec<_> = left.split_whitespace().collect();
            (right.split_whitespace().next() == Some("cgroup2") && fields[4] == ROOT)
                .then_some(fields[3])
        })
        .collect();
    assert_eq!(matches, ["/"], "complete host cgroup2 mount required");
    assert_eq!(fs::canonicalize(ROOT).unwrap(), Path::new(ROOT));
    // A private cgroup subtree can report mount root '/', but has these limits.
    assert!(!Path::new(ROOT).join("cpu.max").exists());
    assert!(!Path::new(ROOT).join("memory.max").exists());
}
fn starttime(pid: u64) -> String {
    let stat = read(format!("/proc/{pid}/stat"));
    let (_, rest) = stat.rsplit_once(") ").unwrap();
    let value = rest.split_whitespace().nth(19).unwrap();
    number(value);
    value.to_owned()
}
fn hierarchy(pid: u64) -> Vec<Value> {
    mount();
    let membership = read(format!("/proc/{pid}/cgroup"));
    let paths: Vec<_> = membership
        .lines()
        .filter_map(|s| s.strip_prefix("0::"))
        .collect();
    assert_eq!(paths.len(), 1);
    let relative = paths[0];
    assert!(relative.starts_with('/') && !relative.contains(['\n', '\r']));
    assert!(relative.split('/').all(|s| !matches!(s, "." | "..")));
    let path = Path::new(ROOT).join(relative.trim_start_matches('/'));
    assert_eq!(fs::canonicalize(&path).unwrap(), path);
    let mut current = path.as_path();
    let mut rows = Vec::new();
    loop {
        assert!(current.starts_with(ROOT));
        let mut values = serde_json::Map::new();
        for field in [
            "cpu.max",
            "cpuset.cpus.effective",
            "memory.max",
            "memory.current",
            "memory.swap.max",
            "memory.swap.current",
            "memory.events",
            "pids.max",
            "pids.current",
            "io.max",
            "cgroup.procs",
        ] {
            let path = current.join(field);
            let value = match fs::metadata(&path) {
                Ok(_) => Value::String(read(&path).trim().to_owned()),
                Err(e) if e.kind() == io::ErrorKind::NotFound && current == Path::new(ROOT) => {
                    Value::Null
                }
                Err(e) => panic!("host cgroup field {}: {e}", path.display()),
            };
            values.insert(field.into(), value);
        }
        rows.push(json!({"path": current, "values": values}));
        if current == Path::new(ROOT) {
            break;
        }
        current = current.parent().unwrap();
    }
    rows
}
fn minimum(values: &Value, cpu: u64, memory: u64, pids: u64, read: u64, write: u64) {
    if let Some(value) = values["cpu.max"].as_str() {
        let fields: Vec<_> = value.split_whitespace().collect();
        assert_eq!(fields.len(), 2);
        let period = number(fields[1]);
        assert!(period > 0);
        assert!(
            fields[0] == "max"
                || u128::from(number(fields[0])) * 100000 >= u128::from(cpu) * u128::from(period),
            "stricter CPU ancestor"
        );
    }
    if let Some(value) = values["memory.max"].as_str() {
        assert!(
            value == "max" || number(value) >= memory,
            "stricter memory ancestor"
        );
    }
    if let Some(value) = values["pids.max"].as_str() {
        assert!(
            value == "max" || number(value) >= pids,
            "stricter PID ancestor"
        );
    }
    let selected = cpus(values["cpuset.cpus.effective"].as_str().unwrap());
    assert!((0..4).all(|cpu| selected.contains(&cpu)));
    if let Some(value) = values["io.max"].as_str() {
        let device =
            std::env::var("SKVOZ_BLOCK_DEVICE_ID").expect("explicit block major:minor required");
        assert!(device.split_once(':').is_some_and(
            |(major, minor)| major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok()
        ));
        for line in value.lines() {
            let mut fields = line.split_whitespace();
            if fields.next() != Some(device.as_str()) {
                continue;
            }
            for field in fields {
                let (name, cap) = field.split_once('=').unwrap();
                let required = match name {
                    "rbps" => read,
                    "wbps" => write,
                    _ => continue,
                };
                assert!(
                    cap == "max" || number(cap) >= required,
                    "stricter IO ancestor"
                );
            }
        }
    }
}
fn proof(role: &str, project: &str, inspect: &Path) -> Value {
    let input = json_file(inspect);
    let input = input.as_array().unwrap();
    assert_eq!(input.len(), 1);
    let input = &input[0];
    let id = input["Id"].as_str().unwrap();
    assert!(
        id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_eq!(
        input["Config"]["Labels"]["com.docker.compose.project"],
        project
    );
    assert_eq!(
        input["Config"]["Labels"]["com.docker.compose.service"],
        role
    );
    assert_eq!(input["State"]["Running"], true);
    let pid = input["State"]["Pid"].as_u64().unwrap();
    assert!(pid > 1);
    let boot = read("/proc/sys/kernel/random/boot_id");
    let start = starttime(pid);
    let status = read(format!("/proc/{pid}/status"));
    let status: std::collections::BTreeMap<_, _> = status
        .lines()
        .filter_map(|line| line.split_once(':').map(|(key, value)| (key, value.trim())))
        .collect();
    let caps = match role {
        "prepare" => 0,
        "endpoint" => 0x1500,
        "server" => 0x11e0,
        "client" => 0x35e0,
        _ => unreachable!(),
    };
    for field in ["CapEff", "CapPrm", "CapBnd"] {
        assert_eq!(u64::from_str_radix(status[field], 16).unwrap(), caps);
    }
    assert_eq!(status["NoNewPrivs"], "1");
    let rows = hierarchy(pid);
    let position = rows
        .iter()
        .position(|row| {
            let name = Path::new(row["path"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            name == id || name == format!("docker-{id}.scope")
        })
        .expect("original Docker ancestor absent");
    let (cpu, memory, pids, rbps, wbps) = profile(role);
    for row in &rows[..=position] {
        minimum(&row["values"], cpu, memory, pids, rbps, wbps);
    }
    let values = &rows[position]["values"];
    assert_eq!(values["cpu.max"], format!("{cpu} 100000"));
    assert_eq!(values["memory.max"], memory.to_string());
    assert_eq!(values["memory.swap.max"], "0");
    assert_eq!(values["memory.swap.current"], "0");
    assert_eq!(values["pids.max"], pids.to_string());
    assert_eq!(values["cpuset.cpus.effective"], "0-3");
    let device = std::env::var("SKVOZ_BLOCK_DEVICE_ID").unwrap();
    let io = values["io.max"].as_str().unwrap();
    let line = io
        .lines()
        .find(|line| line.split_whitespace().next() == Some(device.as_str()))
        .expect("device IO cap absent");
    let limits: std::collections::BTreeMap<_, _> = line
        .split_whitespace()
        .skip(1)
        .map(|field| field.split_once('=').unwrap())
        .collect();
    assert_eq!(limits.get("rbps").copied(), Some(rbps.to_string().as_str()));
    assert_eq!(limits.get("wbps").copied(), Some(wbps.to_string().as_str()));
    for field in ["riops", "wiops"] {
        assert!(limits.get(field).is_none_or(|value| *value == "max"));
    }
    let events: std::collections::BTreeMap<_, _> = values["memory.events"]
        .as_str()
        .unwrap()
        .lines()
        .map(|line| line.split_once(' ').unwrap())
        .collect();
    assert_eq!(events.get("oom"), Some(&"0"));
    assert_eq!(events.get("oom_kill"), Some(&"0"));
    // Inspect actual Docker parents, not the caller's potentially different ancestry.
    let available = read("/proc/meminfo")
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemAvailable:")
                .map(|s| number(s.split_whitespace().next().unwrap()) * 1024)
        })
        .unwrap();
    assert!(available >= 6 * GIB);
    for row in &rows[position + 1..] {
        let values = &row["values"];
        minimum(values, 800000, 6 * GIB, 256, 8388608, 2097152);
        assert!(cpus(values["cpuset.cpus.effective"].as_str().unwrap()).len() >= 8);
        if let Some(limit) = values["memory.max"].as_str().filter(|s| *s != "max") {
            assert!(
                number(limit).saturating_sub(number(values["memory.current"].as_str().unwrap()))
                    >= 6 * GIB,
                "parent half-memory capacity exhausted"
            );
        }
    }
    assert_eq!(starttime(pid), start, "payload PID reused during proof");
    assert_eq!(read("/proc/sys/kernel/random/boot_id"), boot);
    json!({"role": role, "project": project, "id": id, "pid": pid, "starttime": start,
        "boot": boot.trim(), "group": rows[position]["path"], "ancestors": rows})
}
pub fn mode(args: &[std::ffi::OsString]) -> bool {
    if args.first().is_some_and(|s| s == "--host-cleanup") {
        assert_eq!(args.len(), 2);
        mount();
        let record = json_file(Path::new(&args[1]));
        assert_eq!(
            read("/proc/sys/kernel/random/boot_id").trim(),
            record["boot"].as_str().unwrap()
        );
        let group = Path::new(record["group"].as_str().unwrap());
        let id = record["id"].as_str().unwrap();
        assert!(
            group.starts_with(ROOT)
                && group.file_name().is_some_and(|name| name == id
                    || name.to_str() == Some(format!("docker-{id}.scope").as_str()))
        );
        match fs::symlink_metadata(group) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            _ => panic!("recorded payload cgroup remains or is unreadable"),
        }
        let parent = group.parent().unwrap();
        assert_eq!(fs::canonicalize(parent).unwrap(), parent);
        let users = read(parent.join("cgroup.procs"));
        for pid in users.split_whitespace() {
            number(pid);
        }
        let pid = record["pid"].as_u64().unwrap();
        match fs::metadata(format!("/proc/{pid}")) {
            Ok(_) => assert_ne!(
                starttime(pid),
                record["starttime"].as_str().unwrap(),
                "original init remains alive"
            ),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::NotFound),
        }
        println!(
            "{}",
            json!({"absent": group, "parent": parent, "parent_processes": users})
        );
        return true;
    }
    let recheck = args.first().is_some_and(|s| s == "--host-recheck");
    if !recheck && args.first().is_none_or(|s| s != "--host-proof") {
        return false;
    }
    assert_eq!(args.len(), 5);
    let result = proof(
        args[1].to_str().unwrap(),
        args[2].to_str().unwrap(),
        Path::new(&args[3]),
    );
    let output = Path::new(&args[4]);
    if recheck {
        let before = json_file(output);
        for key in ["role", "project", "id", "pid", "starttime", "boot", "group"] {
            assert_eq!(before[key], result[key], "held actor identity changed");
        }
    } else {
        fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
    true
}

pub fn hold(role: &str) {
    fs::write("/run/ip-start-ready", role).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(240);
    while !Path::new("/run/ip-start-release").exists() {
        assert!(
            std::time::Instant::now() < until,
            "host startup proof deadline"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
