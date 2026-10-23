#![cfg(feature = "fixture")]
//! The owning language test manages the real private kernel/IP dependencies.
use futures_util::FutureExt;
use serde_json::Value;
use skvoz_network_contracts::SessionId;
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use testcontainers::{
    ContainerAsync, GenericBuildableImage, GenericImage, ImageExt,
    bollard::{
        Docker,
        errors::Error::DockerResponseServerError,
        models::{
            DeviceMapping, EndpointIpamConfig, EndpointSettings, Ipam, IpamConfig,
            NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
            VolumeCreateRequest,
        },
        query_parameters::{
            ListContainersOptionsBuilder, ListNetworksOptionsBuilder, ListVolumesOptionsBuilder,
            RemoveContainerOptionsBuilder, RemoveImageOptions,
        },
    },
    core::{AccessMode, CmdWaitFor, ExecCommand, ExecResult, Mount},
    runners::{AsyncBuilder, AsyncRunner},
};
use tokio::{io::AsyncBufReadExt, process::Child};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const LABEL: &str = "skvoz.network.test";
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove private test scratch");
    }
}
struct Topology {
    docker: Docker,
    token: String,
    scratch: Scratch,
    containers: Vec<ContainerAsync<GenericImage>>,
    volume: Option<String>,
    compiler: Option<Child>,
}
impl Topology {
    fn new() -> Result<Self> {
        let token = SessionId::random()?.as_str().to_owned();
        let docker = Docker::connect_with_defaults()?;
        let root = std::env::temp_dir().join(format!("network-kernel-{token}"));
        fs::create_dir(&root)?;
        let scratch = Scratch(root);
        fs::set_permissions(&scratch.0, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            docker,
            token,
            scratch,
            containers: vec![],
            volume: None,
            compiler: None,
        })
    }
    fn filters(&self) -> HashMap<String, Vec<String>> {
        HashMap::from([("label".into(), vec![format!("{LABEL}={}", self.token)])])
    }
    async fn network(&mut self, role: &str, v4: &str, v6: &str) -> Result<String> {
        // Testcontainers exposes one named network, but this contract needs
        // explicit dual-stack IPAM and two independently attached networks.
        let response = self
            .docker
            .create_network(NetworkCreateRequest {
                name: format!("network-{}-{role}", self.token),
                driver: Some("bridge".into()),
                internal: Some(true),
                enable_ipv6: Some(true),
                labels: Some(HashMap::from([(LABEL.into(), self.token.clone())])),
                ipam: Some(Ipam {
                    config: Some(
                        [v4, v6]
                            .map(|subnet| IpamConfig {
                                subnet: Some(subnet.into()),
                                ..Default::default()
                            })
                            .to_vec(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?;
        Ok(response.id)
    }
    async fn actor(&mut self, role: &str, bin: &Path, native: &Path) -> Result<usize> {
        let mut request = GenericImage::new("skvoz-network-tests", &self.token)
            .with_entrypoint("/usr/bin/sleep")
            .with_cmd(["infinity"])
            .with_network("none")
            .with_label(LABEL, &self.token)
            .with_readonly_rootfs(true)
            .with_user("0:0")
            .with_security_opt("no-new-privileges:true")
            .with_cap_drop("ALL")
            .with_ulimit("core", 0, Some(0))
            .with_ulimit("nofile", 8192, Some(8192))
            .with_mount(
                Mount::bind_mount(bin.to_str().unwrap(), "/fixture/bin")
                    .with_access_mode(AccessMode::ReadOnly),
            )
            .with_mount(
                Mount::bind_mount(native.to_str().unwrap(), "/opt/skvoz-native")
                    .with_access_mode(AccessMode::ReadOnly),
            )
            .with_env_var("SKVOZ_KERNEL_FIXTURE", "1")
            .with_env_var(
                "SKVOZ_HELPER_EXECUTABLE",
                "/opt/skvoz-native/bin/skvoz-network-helper",
            );
        request = request.with_mount(
            Mount::bind_mount(
                bin.join("nats-server").to_str().unwrap(),
                "/usr/local/bin/nats-server",
            )
            .with_access_mode(AccessMode::ReadOnly),
        );
        let caps: &[&str] = match role {
            "prepare" => &[],
            "endpoint" => &["NET_ADMIN", "NET_BIND_SERVICE", "SETPCAP"],
            "server" => &["NET_ADMIN", "KILL", "SETUID", "SETGID", "SETPCAP"],
            "kernel" | "client" => &[
                "NET_ADMIN",
                "KILL",
                "SETUID",
                "SETGID",
                "SETPCAP",
                "NET_RAW",
                "NET_BIND_SERVICE",
            ],
            _ => unreachable!(),
        };
        for cap in caps {
            request = request.with_cap_add(*cap);
        }
        if role == "prepare" {
            request = request.with_mount(Mount::volume_mount(
                self.volume.as_ref().unwrap(),
                "/outputs",
            ));
        } else if role != "kernel" {
            request = request.with_mount(
                Mount::volume_mount(self.volume.as_ref().unwrap(), "/fixture/input")
                    .with_access_mode(AccessMode::ReadOnly),
            );
        }
        let policy = Path::new(env!("CARGO_MANIFEST_DIR")).join("polkit/org.skvoz.network.policy");
        request = request.with_mount(
            Mount::bind_mount(
                policy.to_str().unwrap(),
                "/usr/share/polkit-1/actions/org.skvoz.network.policy",
            )
            .with_access_mode(AccessMode::ReadOnly),
        );
        request = request.with_mount(
            Mount::bind_mount(
                bin.join("polkit.rules").to_str().unwrap(),
                "/etc/polkit-1/rules.d/49-skvoz-fixture.rules",
            )
            .with_access_mode(AccessMode::ReadOnly),
        );
        let kernel = matches!(role, "kernel" | "server" | "client");
        let forwarding = matches!(role, "kernel" | "server");
        let server = role == "server";
        request = request.with_host_config_modifier(move |host| {
            host.init = Some(true);
            let mut tmpfs = HashMap::from([
                ("/run".into(), "rw,nosuid,nodev,size=64m,mode=0755".into()),
                ("/tmp".into(), "rw,nosuid,nodev,size=64m,mode=1777".into()),
            ]);
            if kernel {
                tmpfs.insert(
                    if server {
                        "/var/lib/skvoz-network"
                    } else {
                        "/var/lib/skvoz-network-helper"
                    }
                    .into(),
                    "rw,nosuid,nodev,size=16m,mode=0700".into(),
                );
                host.devices = Some(vec![DeviceMapping {
                    path_on_host: Some("/dev/net/tun".into()),
                    path_in_container: Some("/dev/net/tun".into()),
                    cgroup_permissions: Some("rwm".into()),
                }]);
                let mut sysctls = HashMap::from([
                    ("net.ipv4.ipfrag_high_thresh".into(), "4194304".into()),
                    ("net.ipv4.ipfrag_time".into(), "15".into()),
                    ("net.ipv6.ip6frag_high_thresh".into(), "4194304".into()),
                    ("net.ipv6.ip6frag_time".into(), "15".into()),
                    (
                        "net.netfilter.nf_conntrack_frag6_high_thresh".into(),
                        "4194304".into(),
                    ),
                    (
                        "net.netfilter.nf_conntrack_frag6_timeout".into(),
                        "15".into(),
                    ),
                ]);
                if forwarding {
                    sysctls.insert("net.ipv4.ip_forward".into(), "1".into());
                    sysctls.insert("net.ipv6.conf.all.forwarding".into(), "1".into());
                }
                host.sysctls = Some(sysctls);
            }
            host.tmpfs = Some(tmpfs);
        });
        let container = request.start().await?;
        self.containers.push(container);
        Ok(self.containers.len() - 1)
    }
    async fn attach(&self, index: usize, network: &str, v4: &str, v6: &str) -> Result<()> {
        let id = self.containers[index].id();
        let settings = self
            .docker
            .inspect_container(id, None)
            .await?
            .network_settings;
        if settings
            .and_then(|s| s.networks)
            .is_some_and(|n| n.contains_key("none"))
        {
            self.docker
                .disconnect_network(
                    "none",
                    NetworkDisconnectRequest {
                        container: id.into(),
                        force: Some(false),
                    },
                )
                .await?;
        }
        self.docker
            .connect_network(
                network,
                NetworkConnectRequest {
                    container: self.containers[index].id().into(),
                    endpoint_config: Some(EndpointSettings {
                        ipam_config: Some(EndpointIpamConfig {
                            ipv4_address: Some(v4.into()),
                            ipv6_address: Some(v6.into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                },
            )
            .await?;
        loop {
            let mut command = self.containers[index]
                .exec(
                    ExecCommand::new(["/usr/sbin/ip", "-6", "-j", "address"])
                        .with_cmd_ready_condition(CmdWaitFor::exit()),
                )
                .await?;
            assert_eq!(command.exit_code().await?, Some(0));
            let addresses: Value = serde_json::from_slice(&command.stdout_to_vec().await?)?;
            let addresses: Vec<&Value> = addresses
                .as_array()
                .ok_or("invalid address snapshot")?
                .iter()
                .flat_map(|link| link["addr_info"].as_array().into_iter().flatten())
                .collect();
            assert!(
                !addresses.iter().any(|a| a["dadfailed"] == true),
                "duplicate IPv6 address"
            );
            if addresses.iter().any(|a| a["local"] == v6)
                && !addresses.iter().any(|a| a["tentative"] == true)
            {
                println!("Namespace IPv6 ready: {v6}; no tentative or DAD-failed addresses");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    async fn checked(&self, index: usize, args: &[&str]) -> Result<String> {
        let mut result = self.containers[index]
            .exec(ExecCommand::new(
                args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ))
            .await?;
        let out = result.stdout_to_vec().await;
        let err = result.stderr_to_vec().await;
        let out = String::from_utf8(out?)?;
        let err = String::from_utf8(err?)?;
        print!("{out}");
        eprint!("{err}");
        assert_eq!(
            result.exit_code().await?,
            Some(0),
            "fixture command {args:?}: {err}"
        );
        Ok(out)
    }
    async fn ready(&self, index: usize, actor: &mut ExecResult) -> Result<()> {
        loop {
            if let Some(code) = actor.exit_code().await? {
                let out = String::from_utf8(actor.stdout_to_vec().await?)?;
                let err = String::from_utf8(actor.stderr_to_vec().await?)?;
                return Err(std::io::Error::other(format!(
                    "actor exited before readiness: code={code}; {out}; {err}"
                ))
                .into());
            }
            let result = self.containers[index]
                .exec(
                    ExecCommand::new(["test", "-f", "/run/topology/ready"])
                        .with_cmd_ready_condition(CmdWaitFor::exit()),
                )
                .await?;
            if result.exit_code().await? == Some(0) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    async fn exercise(&mut self) -> Result<()> {
        let native = self.scratch.0.join("native");
        let bin = self.scratch.0.join("bin");
        fs::create_dir(&native)?;
        fs::create_dir(&bin)?;
        fs::set_permissions(&native, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755))?;
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(
                std::env::var_os("SKVOZ_TEST_READY_NATIVE_ARCHIVE")
                    .ok_or("ready native archive required")?,
            )
            .arg("-C")
            .arg(&native)
            .arg("--no-same-owner")
            .status()?;
        assert!(status.success());
        for (source, name) in [
            (
                PathBuf::from(env!("CARGO_BIN_EXE_fixture-ip-topology")),
                "ip_topology",
            ),
            (
                PathBuf::from(env!("CARGO_BIN_EXE_fixture-kernel-client")),
                "kernel_client",
            ),
            (skvoz_network_qualification::nats_binary(), "nats-server"),
        ] {
            fs::copy(source, bin.join(name))?;
        }
        let mut compiler = tokio::process::Command::new(env!("CARGO"))
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
            .args([
                "test",
                "-p",
                "skvoz-network-native",
                "-p",
                "skvoz-network-helper",
                "--features",
                "skvoz-network-helper/fixture",
                "--test",
                "linux_tun",
                "--test",
                "kernel_lifecycle",
                "--release",
                "--locked",
                "--no-run",
                "--message-format=json",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()?;
        let mut lines = tokio::io::BufReader::new(compiler.stdout.take().unwrap()).lines();
        self.compiler = Some(compiler);
        let mut tests = HashMap::new();
        while let Some(line) = lines.next_line().await? {
            let artifact: Value = serde_json::from_str(&line)?;
            if artifact["reason"] == "compiler-artifact"
                && let Some(name @ ("linux_tun" | "kernel_lifecycle")) =
                    artifact["target"]["name"].as_str()
                && let Some(executable) = artifact["executable"].as_str()
            {
                assert!(
                    tests
                        .insert(name.to_owned(), executable.to_owned())
                        .is_none()
                );
            }
        }
        assert!(self.compiler.as_mut().unwrap().wait().await?.success());
        self.compiler = None;
        for name in ["linux_tun", "kernel_lifecycle"] {
            fs::copy(
                tests.remove(name).ok_or("missing owning test executable")?,
                bin.join(name),
            )?;
        }
        let _ = GenericBuildableImage::new("skvoz-network-tests", &self.token)
            .with_dockerfile_string(include_str!("../../../docker/rust/Dockerfile"))
            .build_image()
            .await?;
        println!(
            "Fixture prepared owning Cargo children; OS-only image {}",
            self.docker
                .inspect_image(&format!("skvoz-network-tests:{}", self.token))
                .await?
                .id
                .ok_or("missing test image identity")?
        );
        fs::write(
            bin.join("polkit.rules"),
            "polkit.addRule(function(action, subject) { if (action.id == \"org.skvoz.network.manage\" && subject.user == \"skvoz\") return polkit.Result.YES; });\n",
        )?;
        fs::set_permissions(bin.join("polkit.rules"), fs::Permissions::from_mode(0o644))?;
        let underlay = self
            .network("underlay", "198.18.0.0/24", "2001:db8:800::/64")
            .await?;
        let egress = self
            .network("egress", "198.19.0.0/24", "2001:db8:900::/64")
            .await?;
        let volume = format!("network-{}-inputs", self.token);
        self.docker
            .create_volume(VolumeCreateRequest {
                name: Some(volume.clone()),
                labels: Some(HashMap::from([(LABEL.into(), self.token.clone())])),
                ..Default::default()
            })
            .await?;
        self.volume = Some(volume);
        let kernel = self.actor("kernel", &bin, &native).await?;
        self.attach(kernel, &egress, "198.19.0.20", "2001:db8:900::20")
            .await?;
        let output = self
            .checked(kernel, &["/fixture/bin/ip_topology", "kernel"])
            .await?;
        assert_eq!(
            output
                .matches("test result: ok. 1 passed; 0 failed")
                .count(),
            2,
            "both real TUN/server helper tests must execute"
        );
        assert!(output.contains("Actual UID/polkit helper guard/DNS/routes/foreign handle/abort/EOF/crash/recovery passed"));
        self.containers.remove(kernel).rm().await?;
        let prepare = self.actor("prepare", &bin, &native).await?;
        self.checked(prepare, &["/fixture/bin/ip_topology", "prepare"])
            .await?;
        self.containers.remove(prepare).rm().await?;
        let endpoint = self.actor("endpoint", &bin, &native).await?;
        self.attach(endpoint, &underlay, "198.18.0.2", "2001:db8:800::2")
            .await?;
        self.attach(endpoint, &egress, "198.19.0.30", "2001:db8:900::30")
            .await?;
        let mut endpoint_run = self.containers[endpoint]
            .exec(ExecCommand::new(["/fixture/bin/ip_topology", "endpoint"]))
            .await?;
        self.ready(endpoint, &mut endpoint_run).await?;
        let server = self.actor("server", &bin, &native).await?;
        self.attach(server, &underlay, "198.18.0.20", "2001:db8:800::20")
            .await?;
        self.attach(server, &egress, "198.19.0.20", "2001:db8:900::20")
            .await?;
        let mut server_run = self.containers[server]
            .exec(ExecCommand::new(["/fixture/bin/ip_topology", "server"]))
            .await?;
        self.ready(server, &mut server_run).await?;
        let client = self.actor("client", &bin, &native).await?;
        self.attach(client, &underlay, "198.18.0.10", "2001:db8:800::10")
            .await?;
        let output = self
            .checked(client, &["/fixture/bin/ip_topology", "client"])
            .await?;
        assert!(output.contains(
            "Frozen runtime/helper/Core NAT44/IPv6/DNS/guard/runtime-crash/recovery passed"
        ));
        self.checked(server, &["/fixture/bin/ip_topology", "--shutdown"])
            .await?;
        let out = server_run.stdout_to_vec().await;
        let err = server_run.stderr_to_vec().await;
        print!("{}", String::from_utf8(out?)?);
        eprint!("{}", String::from_utf8(err?)?);
        assert_eq!(
            server_run.exit_code().await?,
            Some(0),
            "real server actor exit and owned cleanup"
        );
        println!("Server actor exit=0; explicit Rust shutdown/reap passed");
        self.containers[endpoint].stop_with_timeout(Some(1)).await?;
        let out = endpoint_run.stdout_to_vec().await;
        let err = endpoint_run.stderr_to_vec().await;
        print!("{}", String::from_utf8(out?)?);
        eprint!("{}", String::from_utf8(err?)?);
        Ok(())
    }
    async fn diagnostics(&self) {
        for container in &self.containers {
            for args in [
                vec!["/usr/sbin/ip", "-6", "-j", "address"],
                vec!["/usr/sbin/ip", "-6", "-j", "route", "show", "table", "all"],
                vec!["/usr/sbin/ip", "-6", "-j", "neighbour"],
                vec![
                    "/usr/sbin/sysctl",
                    "net.ipv6.conf.all.forwarding",
                    "net.ipv6.conf.default.forwarding",
                    "net.ipv6.conf.eth0.forwarding",
                    "net.ipv6.conf.eth1.forwarding",
                    "net.ipv6.conf.all.disable_ipv6",
                    "net.ipv6.conf.default.disable_ipv6",
                    "net.ipv6.conf.eth0.disable_ipv6",
                    "net.ipv6.conf.eth1.disable_ipv6",
                ],
            ] {
                let result = container
                    .exec(
                        ExecCommand::new(args.clone()).with_cmd_ready_condition(CmdWaitFor::exit()),
                    )
                    .await;
                match result {
                    Ok(mut command) => {
                        let out = command.stdout_to_vec().await;
                        let err = command.stderr_to_vec().await;
                        eprintln!(
                            "Namespace diagnostic {} {args:?}: stdout={}; stderr={}",
                            container.id(),
                            out.map(|b| String::from_utf8_lossy(&b).into_owned())
                                .unwrap_or_else(|e| e.to_string()),
                            err.map(|b| String::from_utf8_lossy(&b).into_owned())
                                .unwrap_or_else(|e| e.to_string())
                        );
                    }
                    Err(error) => eprintln!("Namespace diagnostic {}: {error}", container.id()),
                }
            }
        }
    }
    async fn cleanup(&mut self) -> Result<()> {
        let mut errors = vec![];
        if let Some(mut compiler) = self.compiler.take() {
            let running = match compiler.try_wait() {
                Ok(status) => status.is_none(),
                Err(e) => {
                    errors.push(format!("compiler status: {e}"));
                    true
                }
            };
            if running {
                // Cargo and its compiler children share this owned process group.
                if let Some(pid) = compiler.id() {
                    let status = Command::new("/bin/kill")
                        .args(["-KILL", "--"])
                        .arg(format!("-{pid}"))
                        .status();
                    if !matches!(status, Ok(status) if status.success()) {
                        errors.push(format!("kill compiler group: {status:?}"));
                    }
                }
                if let Err(e) = compiler.kill().await {
                    errors.push(format!("kill and reap compiler: {e}"));
                }
            }
        }
        while let Some(container) = self.containers.pop() {
            if let Err(e) = container.rm().await {
                errors.push(e.to_string());
            }
        }
        // Sweep all three kinds by the unique ownership label, including
        // a daemon create completed before a cancelled response was recorded.
        let filters = self.filters();
        match self
            .docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await
        {
            Ok(containers) => {
                for c in containers {
                    if let Some(id) = c.id
                        && let Err(e) = self
                            .docker
                            .remove_container(
                                &id,
                                Some(
                                    RemoveContainerOptionsBuilder::default()
                                        .force(true)
                                        .v(true)
                                        .build(),
                                ),
                            )
                            .await
                    {
                        errors.push(format!("remove container {id}: {e}"));
                    }
                }
            }
            Err(e) => errors.push(format!("list owned containers: {e}")),
        }
        match self
            .docker
            .list_networks(Some(
                ListNetworksOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
        {
            Ok(networks) => {
                for n in networks {
                    if let Some(id) = n.id
                        && let Err(e) = self.docker.remove_network(&id).await
                    {
                        errors.push(format!("remove network {id}: {e}"));
                    }
                }
            }
            Err(e) => errors.push(format!("list owned networks: {e}")),
        }
        match self
            .docker
            .list_volumes(Some(
                ListVolumesOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
        {
            Ok(response) => {
                for volume in response.volumes.unwrap_or_default() {
                    if let Err(e) = self
                        .docker
                        .remove_volume(
                            &volume.name,
                            None::<testcontainers::bollard::query_parameters::RemoveVolumeOptions>,
                        )
                        .await
                    {
                        errors.push(format!("remove volume {}: {e}", volume.name));
                    }
                }
            }
            Err(e) => errors.push(format!("list owned volumes: {e}")),
        }
        match self
            .docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .all(true)
                    .filters(&filters)
                    .build(),
            ))
            .await
        {
            Ok(v) if v.is_empty() => {}
            v => errors.push(format!("container absence: {v:?}")),
        }
        match self
            .docker
            .list_networks(Some(
                ListNetworksOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
        {
            Ok(v) if v.is_empty() => {}
            v => errors.push(format!("network absence: {v:?}")),
        }
        match self
            .docker
            .list_volumes(Some(
                ListVolumesOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
        {
            Ok(v) if v.volumes.as_ref().is_none_or(Vec::is_empty) => {}
            v => errors.push(format!("volume absence: {v:?}")),
        }
        let image = format!("skvoz-network-tests:{}", self.token);
        match self
            .docker
            .remove_image(&image, None::<RemoveImageOptions>, None)
            .await
        {
            Ok(_)
            | Err(DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(e) => errors.push(format!("remove image {image}: {e}")),
        }
        match self.docker.inspect_image(&image).await {
            Err(DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            result => errors.push(format!("image absence {image}: {result:?}")),
        }
        if !errors.is_empty() {
            return Err(std::io::Error::other(errors.join("; ")).into());
        }
        println!(
            "Rust test cleanup verified: no owned compiler, image, containers, networks or volumes; label={}",
            self.token
        );
        Ok(())
    }
}
#[tokio::test]
#[ignore = "requires Docker/TUN and exact ready Linux candidate"]
async fn real_kernel_helper_and_dual_stack_ip_preserve_owned_lifecycle() {
    let mut topology = Topology::new().unwrap();
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(300),
        topology.exercise(),
    ))
    .catch_unwind()
    .await;
    let cleanup_end = tokio::time::Instant::now() + Duration::from_secs(90);
    if !matches!(&result, Ok(Ok(Ok(())))) {
        let _ = tokio::time::timeout(Duration::from_secs(5), topology.diagnostics()).await;
    }
    let cleanup = tokio::time::timeout_at(cleanup_end, topology.cleanup()).await;
    cleanup
        .expect("kernel fixture cleanup deadline")
        .expect("kernel fixture cleanup failed");
    match result {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => panic!("kernel fixture failed: {e}"),
        Ok(Err(e)) => panic!("kernel fixture deadline: {e}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
