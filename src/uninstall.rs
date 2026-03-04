use std::process::Command;

// ============================================================
//  WINDOWS
// ============================================================

#[cfg(target_os = "windows")]
fn is_admin() -> bool {
    Command::new("net").args(["session"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn main() {
    use std::os::windows::process::CommandExt;

    // Auto-elevação UAC
    if !is_admin() {
        let exe = std::env::current_exe().unwrap();
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command",
                &format!("Start-Process -FilePath '{}' -Verb RunAs -Wait", exe.display())
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .status();
        return;
    }

    println!("Desinstalando Agente MAC/Hostname...");
    println!();

    // Parar serviço
    print!("[...] Parando serviço...");
    let _ = Command::new("sc").args(["stop", "AgenteMac"]).output();
    std::thread::sleep(std::time::Duration::from_secs(2));
    println!("\r[OK] Serviço parado        ");

    // Remover serviço
    print!("[...] Removendo serviço...");
    let _ = Command::new("sc").args(["delete", "AgenteMac"]).output();
    std::thread::sleep(std::time::Duration::from_secs(1));
    println!("\r[OK] Serviço removido      ");

    // Remover arquivos
    let install_dir = std::path::Path::new("C:\\AgenteMac");
    if install_dir.exists() {
        print!("[...] Removendo arquivos...");
        let _ = std::fs::remove_dir_all(install_dir);
        println!("\r[OK] Arquivos removidos    ");
    } else {
        println!("[INFO] Pasta de instalação não encontrada");
    }

    println!();
    println!("============================================");
    println!(" Desinstalação concluída!");
    println!("============================================");
    println!();
    println!("Pressione Enter para sair...");
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
}

// ============================================================
//  LINUX
// ============================================================

#[cfg(not(target_os = "windows"))]
fn geteuid() -> u32 {
    unsafe extern "C" { fn geteuid() -> u32; }
    unsafe { geteuid() }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    // Auto-elevação sudo
    if geteuid() != 0 {
        let exe = std::env::current_exe().unwrap();
        println!("Solicitando permissão de administrador...");
        let status = Command::new("sudo")
            .arg(exe.to_str().unwrap())
            .status();
        if let Ok(s) = status {
            std::process::exit(s.code().unwrap_or(0));
        }
        eprintln!("Erro: execute com sudo para desinstalar.");
        std::process::exit(1);
    }

    println!();
    println!("============================================");
    println!(" Desinstalando Agente MAC/Hostname...");
    println!("============================================");
    println!();

    // Parar serviço
    print!("[...] Parando serviço...");
    let _ = Command::new("systemctl").args(["stop", "agentemac"]).output();
    println!("\r[OK] Serviço parado        ");

    // Desabilitar do boot
    print!("[...] Desabilitando serviço...");
    let _ = Command::new("systemctl").args(["disable", "agentemac"]).output();
    println!("\r[OK] Serviço desabilitado  ");

    // Remover arquivo de serviço
    let service_file = "/etc/systemd/system/agentemac.service";
    if std::path::Path::new(service_file).exists() {
        let _ = std::fs::remove_file(service_file);
    }
    let _ = Command::new("systemctl").args(["daemon-reload"]).output();
    println!("[OK] Arquivo de serviço removido");

    // Remover arquivos
    let install_dir = "/opt/agentemac";
    if std::path::Path::new(install_dir).exists() {
        let _ = std::fs::remove_dir_all(install_dir);
        println!("[OK] Arquivos removidos de {}", install_dir);
    } else {
        println!("[INFO] Pasta de instalação não encontrada");
    }

    println!();
    println!("============================================");
    println!(" Desinstalação concluída!");
    println!("============================================");
    println!();
    println!("Pressione Enter para sair...");
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
}