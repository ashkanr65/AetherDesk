use std::io::{self, Write};
use ad_ssh::SshClient;
use ad_config::SshConfig;
use ad_events::EventBus;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🖥️  AetherDesk CLI Client");
    println!("========================\n");
    
    // Get server address
    print!("Server address (e.g., 192.168.1.5:22): ");
    io::stdout().flush()?;
    let mut address = String::new();
    io::stdin().read_line(&mut address)?;
    let address = address.trim();
    
    // Get username
    print!("Username: ");
    io::stdout().flush()?;
    let mut username = String::new();
    io::stdin().read_line(&mut username)?;
    let username = username.trim();
    
    // Get password
    print!("Password: ");
    io::stdout().flush()?;
    let password = rpassword::read_password()?;
    println!();
    
    // Create event bus
    let event_bus = Arc::new(EventBus::new(100));
    
    // Parse host and port
    let (host, port) = if let Some(parts) = address.split_once(':') {
        (parts.0, parts.1.parse().unwrap_or(22))
    } else {
        (address, 22u16)
    };
    
    // Create SSH config
    let config = SshConfig {
        host: host.to_string(),
        port,
        username: username.to_string(),
        password: Some(password),
        ..Default::default()
    };
    
    println!("\n🔌 Connecting to {}:{}...", config.host, config.port);
    
    // Create and connect SSH client
    let client = SshClient::new(config, event_bus.clone());
    
    match client.connect().await {
        Ok(mut session) => {
            println!("✅ Connected successfully!");
            println!("📡 Type commands or 'quit' to exit\n");
            
            loop {
                print!("$ ");
                io::stdout().flush()?;
                
                let mut command = String::new();
                io::stdin().read_line(&mut command)?;
                let command = command.trim();
                
                if command == "quit" || command == "exit" {
                    break;
                }
                
                if !command.is_empty() {
                    // Execute command via SSH
                    match session.execute_command(command).await {
                        Ok(output) => {
                            println!("{}", output);
                        }
                        Err(e) => {
                            eprintln!("❌ Error: {}", e);
                        }
                    }
                }
            }
            
            session.disconnect().await?;
            println!("\n👋 Disconnected");
        }
        Err(e) => {
            eprintln!("❌ Connection failed: {}", e);
        }
    }
    
    Ok(())
}
