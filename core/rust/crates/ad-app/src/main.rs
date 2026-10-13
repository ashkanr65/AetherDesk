//! AetherDesk - Secure Remote Desktop Application
//! 
//! Main entry point for the AetherDesk application.
//! This binary initializes the core systems and starts the application.

use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn init_logging() {
    // Use try_init to avoid panic if already initialized
    let _ = tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .try_init();
}

fn main() -> Result<()> {
    // Initialize logging
    init_logging();
    
    tracing::info!("Starting AetherDesk v{}", env!("CARGO_PKG_VERSION"));
    
    // Initialize the core library
    let result = ad_app::ad_initialize();
    if result != 0 {
        tracing::error!("Failed to initialize AetherDesk core");
        std::process::exit(1);
    }
    
    tracing::info!("AetherDesk core initialized successfully");
    
    // In a full implementation, this would:
    // 1. Load configuration
    // 2. Start the UI (via Flutter FFI or native)
    // 3. Initialize session management
    // 4. Start background services
    
    println!("AetherDesk v{} - Secure Remote Desktop", env!("CARGO_PKG_VERSION"));
    println!("Core initialized. Ready for connections.");
    println!("\nThis is the CLI backend. Use the Flutter UI for full functionality.");
    println!("Or use the FFI library (ad_app.dll/.so/.dylib) from your application.");
    
    Ok(())
}
