use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use std::process::Command;

#[derive(Debug)]
struct MacInfo {
    interface: String,
    mac: String,
    ip: String,
}

// ============================================================
//  HOSTNAME
// ============================================================

fn get_hostname() -> String {
    #[cfg(not(target_os = "windows"))]
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() { return h; }
    }

    #[cfg(not(target_os = "windows"))]
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() { return h; }
    }

    #[cfg(target_os = "windows")]
    if let Ok(out) = Command::new("hostname").output() {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }

    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

// ============================================================
//  MAC ADDRESSES
// ============================================================

#[cfg(target_os = "windows")]
fn get_mac_addresses() -> Vec<MacInfo> {
    let mut macs = Vec::new();

    let result = Command::new("powershell")
        .args(["-NoProfile", "-Command", r#"
            Get-NetAdapter | Where-Object { $_.Status -eq 'Up' } | ForEach-Object {
                $ip = (Get-NetIPAddress -InterfaceIndex $_.InterfaceIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue).IPAddress
                Write-Output "$($_.Name)|$($_.MacAddress)|$ip"
            }
        "#])
        .output();

    if let Ok(out) = result {
        let data = String::from_utf8_lossy(&out.stdout);
        for line in data.lines() {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() == 3 {
                let mac = parts[1].trim().replace('-', ":").to_lowercase();
                let ip = parts[2].trim().to_string();
                if !mac.is_empty() && mac != "00:00:00:00:00:00" {
                    macs.push(MacInfo {
                        interface: parts[0].trim().to_string(),
                        mac,
                        ip,
                    });
                }
            }
        }
    }

    macs
}

#[cfg(not(target_os = "windows"))]
fn get_mac_addresses() -> Vec<MacInfo> {
    let mut macs = Vec::new();

    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let iface = entry.file_name().to_string_lossy().to_string();
            if iface == "lo" { continue; }

            let mac_path = format!("/sys/class/net/{}/address", iface);
            let mac = match std::fs::read_to_string(&mac_path) {
                Ok(m) => m.trim().to_string(),
                Err(_) => continue,
            };
            if mac == "00:00:00:00:00:00" || mac.is_empty() { continue; }

            let ip = Command::new("ip")
                .args(["addr", "show", &iface])
                .output()
                .ok()
                .and_then(|out| {
                    let text = String::from_utf8_lossy(&out.stdout).to_string();
                    text.lines()
                        .find(|l| l.trim().starts_with("inet "))
                        .and_then(|l| l.trim().split_whitespace().nth(1))
                        .and_then(|cidr| cidr.split('/').next())
                        .map(|s| s.to_string())
                })
                .unwrap_or_default();

            macs.push(MacInfo { interface: iface, mac, ip });
        }
    }

    macs
}

// ============================================================
//  HTTP SERVER
// ============================================================

fn macs_to_json(macs: &[MacInfo]) -> String {
    let items: Vec<String> = macs.iter().map(|m| {
        format!(r#"{{"interface":"{}","mac":"{}","ip":"{}"}}"#, m.interface, m.mac, m.ip)
    }).collect();
    format!("[{}]", items.join(","))
}

fn handle_client(mut stream: TcpStream) {
    let mut buf = [0u8; 1024];
    if stream.read(&mut buf).is_err() { return; }

    let request = String::from_utf8_lossy(&buf);
    let first_line = request.lines().next().unwrap_or("");
    let is_options = first_line.starts_with("OPTIONS");
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
    let cors = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\n";

    if is_options {
        let _ = stream.write_all(
            format!("HTTP/1.1 204 No Content\r\n{}Content-Length: 0\r\n\r\n", cors).as_bytes()
        );
        return;
    }

    if path == "/mac" {
        let body = format!(
            r#"{{"hostname":"{}","macs":{}}}"#,
            get_hostname(),
            macs_to_json(&get_mac_addresses())
        );
        let _ = stream.write_all(format!(
            "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            cors, body.len(), body
        ).as_bytes());
    } else {
        let body = r#"{"error":"Not found"}"#;
        let _ = stream.write_all(format!(
            "HTTP/1.1 404 Not Found\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            cors, body.len(), body
        ).as_bytes());
    }
}

fn run_server() {
    let addr = "127.0.0.1:6060";
    let listener = TcpListener::bind(addr).unwrap_or_else(|_| {
        kill_port();
        TcpListener::bind(addr).expect("Falha ao iniciar o servidor após liberar a porta")
    });
    println!("Agente rodando em http://{}/mac", addr);
    for stream in listener.incoming().flatten() {
        std::thread::spawn(|| handle_client(stream));
    }
}

fn kill_port() {
    #[cfg(target_os = "windows")]
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-Command",
            "Get-NetTCPConnection -LocalPort 6060 -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.OwningProcess -Force -ErrorAction SilentlyContinue }"
        ]).output();

    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("fuser").args(["-k", "6060/tcp"]).output();
        if let Ok(out) = Command::new("lsof").args(["-ti", "tcp:6060"]).output() {
            for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                let _ = Command::new("kill").args(["-9", pid]).output();
            }
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(300));
}

// ============================================================
//  AUTO INSTALL / SERVICE
// ============================================================

#[cfg(target_os = "windows")]
fn is_running_as_service() -> bool {
    // Quando rodado como serviço, não há console interativo
    std::env::var("SESSIONNAME").is_err()
}

#[cfg(target_os = "windows")]
fn self_install() {
    use std::os::windows::process::CommandExt;

    let exe = std::env::current_exe().expect("Não foi possível obter o caminho do executável");
    let install_dir = std::path::Path::new("C:\\AgenteMac");
    let dest = install_dir.join("agent.exe");

    // Verifica se já está instalado no local correto
    if exe == dest {
        // Já está no lugar certo, só garante que o serviço existe
        ensure_service(&dest.to_string_lossy());
        return;
    }

    // Re-executa como Administrador via PowerShell (UAC)
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command",
            &format!(
                "Start-Process -FilePath '{}' -Verb RunAs -Wait",
                exe.display()
            )
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .status();

    // Se conseguiu elevar, o processo elevado faz o resto
    match status {
        Ok(s) if s.success() => std::process::exit(0),
        _ => {
            // Fallback: tenta direto (pode falhar sem admin)
            do_install(&exe, install_dir, &dest);
        }
    }
}

#[cfg(target_os = "windows")]
fn do_install(exe: &std::path::Path, install_dir: &std::path::Path, dest: &std::path::Path) {
    // Criar pasta
    let _ = std::fs::create_dir_all(install_dir);

    // Copiar executável
    if let Err(e) = std::fs::copy(exe, dest) {
        eprintln!("Erro ao copiar executável: {}", e);
        std::process::exit(1);
    }

    // Parar e remover serviço anterior
    let _ = Command::new("sc").args(["stop", "AgenteMac"]).output();
    std::thread::sleep(std::time::Duration::from_millis(2000));
    let _ = Command::new("sc").args(["delete", "AgenteMac"]).output();
    std::thread::sleep(std::time::Duration::from_millis(1000));

    ensure_service(&dest.to_string_lossy());

    println!("Instalação concluída! O agente está rodando em http://127.0.0.1:6060/mac");
    std::thread::sleep(std::time::Duration::from_secs(3));
}

#[cfg(target_os = "windows")]
fn ensure_service(exe_path: &str) {
    // Criar serviço
    let _ = Command::new("sc")
        .args(["create", "AgenteMac",
            "binPath=", &format!("\"{}\" --run", exe_path),
            "DisplayName=", "Agente MAC/Hostname",
            "start=", "auto",
            "obj=", "LocalSystem"
        ]).output();

    // Reinício automático em falha
    let _ = Command::new("sc")
        .args(["failure", "AgenteMac",
            "reset=", "60",
            "actions=", "restart/3000/restart/5000/restart/10000"
        ]).output();

    // Iniciar serviço
    let _ = Command::new("sc").args(["start", "AgenteMac"]).output();
}

#[cfg(not(target_os = "windows"))]
fn self_install() {
    let exe = std::env::current_exe().expect("Não foi possível obter o caminho do executável");
    let install_dir = std::path::Path::new("/opt/agentemac");
    let dest = install_dir.join("agent");

    // Já está instalado no lugar certo
    if exe == dest {
        ensure_service(&dest.to_string_lossy());
        return;
    }

    // Tenta instalar direto; se falhar, pede sudo
    if std::fs::create_dir_all(install_dir).is_err() || std::fs::copy(&exe, &dest).is_err() {
        // Re-executa com sudo
        let status = Command::new("sudo")
            .arg(exe.to_str().unwrap())
            .status();
        if let Ok(s) = status {
            std::process::exit(s.code().unwrap_or(0));
        }
        eprintln!("Erro: execute com sudo para instalar.");
        std::process::exit(1);
    }

    let _ = Command::new("chmod").args(["+x", &dest.to_string_lossy()]).output();
    ensure_service(&dest.to_string_lossy());

    println!("Instalação concluída! O agente está rodando em http://127.0.0.1:6060/mac");
}

#[cfg(not(target_os = "windows"))]
fn ensure_service(exe_path: &str) {
    let service = format!(
        "[Unit]\nDescription=Agente MAC/Hostname\nAfter=network.target\nStartLimitIntervalSec=0\n\n\
         [Service]\nType=simple\nExecStart={}\nRestart=always\nRestartSec=3\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        exe_path
    );

    let _ = std::fs::write("/etc/systemd/system/agentemac.service", service);
    let _ = Command::new("systemctl").args(["daemon-reload"]).output();
    let _ = Command::new("systemctl").args(["enable", "agentemac"]).output();
    let _ = Command::new("systemctl").args(["restart", "agentemac"]).output();
}

// ============================================================
//  MAIN
// ============================================================


// ============================================================
//  UNINSTALL
// ============================================================

#[cfg(target_os = "windows")]
fn self_uninstall() {
    use std::os::windows::process::CommandExt;

    // Re-executa como Administrador se necessário
    let exe = std::env::current_exe().unwrap();
    let is_admin = Command::new("net").args(["session"]).output()
        .map(|o| o.status.success()).unwrap_or(false);

    if !is_admin {
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command",
                &format!("Start-Process -FilePath '{}' -ArgumentList '--uninstall' -Verb RunAs -Wait", exe.display())
            ])
            .creation_flags(0x08000000)
            .status();
        return;
    }

    println!("Parando e removendo serviço...");
    let _ = Command::new("sc").args(["stop", "AgenteMac"]).output();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = Command::new("sc").args(["delete", "AgenteMac"]).output();
    std::thread::sleep(std::time::Duration::from_secs(1));

    let install_dir = std::path::Path::new("C:\\AgenteMac");
    if install_dir.exists() {
        let _ = std::fs::remove_dir_all(install_dir);
    }

    println!("Desinstalação concluída!");
    std::thread::sleep(std::time::Duration::from_secs(3));
}

#[cfg(not(target_os = "windows"))]
fn self_uninstall() {
    // Re-executa com sudo se necessário
    if unsafe { libc_geteuid() } != 0 {
        let exe = std::env::current_exe().unwrap();
        let _ = Command::new("sudo")
            .args([exe.to_str().unwrap(), "--uninstall"])
            .status();
        return;
    }

    println!("Parando e removendo serviço...");
    let _ = Command::new("systemctl").args(["stop", "agentemac"]).output();
    let _ = Command::new("systemctl").args(["disable", "agentemac"]).output();
    let _ = std::fs::remove_file("/etc/systemd/system/agentemac.service");
    let _ = Command::new("systemctl").args(["daemon-reload"]).output();
    let _ = std::fs::remove_dir_all("/opt/agentemac");

    println!("Desinstalação concluída!");
}

#[cfg(not(target_os = "windows"))]
fn libc_geteuid() -> u32 {
    unsafe extern "C" { fn geteuid() -> u32; }
    unsafe { geteuid() }
}

fn is_already_installed() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };

    #[cfg(target_os = "windows")]
    return exe.starts_with("C:\\AgenteMac");

    #[cfg(not(target_os = "windows"))]
    return exe.starts_with("/opt/agentemac");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(|s| s.as_str()) == Some("--run") {
        run_server();
        return;
    }

    if args.get(1).map(|s| s.as_str()) == Some("--uninstall") {
        self_uninstall();
        return;
    }

    #[cfg(target_os = "windows")]
    {
        if is_running_as_service() {
            run_server();
            return;
        }
    }

    // Se já está no diretório de instalação, só roda o servidor
    if is_already_installed() {
        run_server();
        return;
    }

    // Primeira execução fora do diretório: instala e registra como serviço
    self_install();
}