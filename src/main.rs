use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use std::process::Command;

#[derive(Debug)]
struct MacInfo {
    interface: String,
    mac: String,
    ip: String,
}

fn get_hostname() -> String {
    // Tenta ler direto do kernel (mais confiável)
    #[cfg(not(target_os = "windows"))]
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() { return h; }
    }

    #[cfg(target_os = "windows")]
    let output = Command::new("hostname").output();

    #[cfg(not(target_os = "windows"))]
    let output = Command::new("hostname").arg("-f").output();

    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => {
            #[cfg(not(target_os = "windows"))]
            if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
                return h.trim().to_string();
            }
            std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "unknown".to_string())
        }
    }
}

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

            if iface == "lo" {
                continue;
            }

            let mac_path = format!("/sys/class/net/{}/address", iface);
            let mac = match std::fs::read_to_string(&mac_path) {
                Ok(m) => m.trim().to_string(),
                Err(_) => continue,
            };

            if mac == "00:00:00:00:00:00" || mac.is_empty() {
                continue;
            }

            let ip_output = Command::new("ip")
                .args(["addr", "show", &iface])
                .output();

            let ip = if let Ok(out) = ip_output {
                let text = String::from_utf8_lossy(&out.stdout);
                text.lines()
                    .find(|l| l.trim().starts_with("inet "))
                    .and_then(|l| l.trim().split_whitespace().nth(1))
                    .and_then(|cidr| cidr.split('/').next())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            };

            macs.push(MacInfo {
                interface: iface,
                mac,
                ip,
            });
        }
    }

    macs
}

fn macs_to_json(macs: &[MacInfo]) -> String {
    let items: Vec<String> = macs
        .iter()
        .map(|m| {
            format!(
                r#"{{"interface":"{}","mac":"{}","ip":"{}"}}"#,
                m.interface, m.mac, m.ip
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn handle_client(mut stream: TcpStream) {
    let mut buf = [0u8; 1024];
    if stream.read(&mut buf).is_err() {
        return;
    }

    let request = String::from_utf8_lossy(&buf);
    let first_line = request.lines().next().unwrap_or("");
    let is_options = first_line.starts_with("OPTIONS");
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");

    let cors = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\n";

    if is_options {
        let response = format!("HTTP/1.1 204 No Content\r\n{}Content-Length: 0\r\n\r\n", cors);
        let _ = stream.write_all(response.as_bytes());
        return;
    }

    if path == "/mac" {
        let hostname = get_hostname();
        let macs = get_mac_addresses();
        let body = format!(
            r#"{{"hostname":"{}","macs":{}}}"#,
            hostname,
            macs_to_json(&macs)
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            cors,
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    } else {
        let body = r#"{"error":"Not found"}"#;
        let response = format!(
            "HTTP/1.1 404 Not Found\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            cors,
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    }
}

fn kill_previous_instance() {
    #[cfg(target_os = "windows")]
    {
        // Mata qualquer processo usando a porta 6060
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command",
                "Get-NetTCPConnection -LocalPort 6060 -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.OwningProcess -Force -ErrorAction SilentlyContinue }"
            ])
            .output();
    }

    #[cfg(not(target_os = "windows"))]
    {
        // fuser -k mata o processo usando a porta
        let _ = Command::new("fuser")
            .args(["-k", "6060/tcp"])
            .output();

        // Fallback: lsof + kill
        if let Ok(out) = Command::new("lsof")
            .args(["-ti", "tcp:6060"])
            .output()
        {
            let pids = String::from_utf8_lossy(&out.stdout);
            for pid in pids.split_whitespace() {
                let _ = Command::new("kill").args(["-9", pid]).output();
            }
        }
    }

    // Pequena pausa para o SO liberar a porta
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn main() {
    let addr = "127.0.0.1:6060";

    // Tenta bind direto; se falhar, mata instância anterior e tenta de novo
    let listener = TcpListener::bind(addr).unwrap_or_else(|_| {
        eprintln!("Porta em uso, encerrando instância anterior...");
        kill_previous_instance();
        TcpListener::bind(addr).expect("Falha ao iniciar o servidor após liberar a porta")
    });

    println!("Agente rodando em http://{}/mac", addr);

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                std::thread::spawn(|| handle_client(s));
            }
            Err(e) => eprintln!("Erro de conexão: {}", e),
        }
    }
}