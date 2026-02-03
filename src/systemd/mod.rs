use std::fs::File;
use std::io::Write;
use std::path::Path;

pub fn generate_service_template(mesh_name: &str, layer_name: &str) -> String {
    format!(
        r#"[Unit]
Description=AIGOS Layer {} (mesh {})
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=aigos
Group=aigos
WorkingDirectory=/var/lib/aigos
EnvironmentFile=-/etc/aigos/env
ExecStart=/usr/local/bin/{} --mesh {}
Restart=on-failure
RestartSec=5
KillMode=mixed
KillSignal=SIGTERM
SendSIGKILL=no
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
"#,
        layer_name, mesh_name, layer_name, mesh_name
    )
}

pub fn write_service_file(output_dir: &Path, mesh_name: &str, layer_name: &str) -> std::io::Result<()> {
    let content = generate_service_template(mesh_name, layer_name);
    let filename = format!("aigosd-{}@{}.service", mesh_name, layer_name);
    let path = output_dir.join(&filename);
    let mut file = File::create(path)?;
    file.write_all(content.as_bytes())?;
    Ok(())
}