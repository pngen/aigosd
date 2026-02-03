use crate::logging;

pub fn get_exe_name(mesh_name: &str, layer_name: &str) -> String {
    format!("aigosd-{}@{}.exe", mesh_name, layer_name)
}

pub fn get_layer_exe_path(layer_name: &str) -> String {
    format!(".\\{}\\{}.exe", layer_name, layer_name)
}

#[cfg(windows)]
pub fn register_service(mesh_name: &str, layer_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let svc_name = format!("aigosd-{}@{}", mesh_name, layer_name);
    let _exe_path = get_layer_exe_path(layer_name);
    logging::info(&format!("Registering Windows service: {}", svc_name));
    // Actual implementation requires windows-service crate
    // sc.exe create {svc_name} binPath= "{exe_path} --mesh {mesh_name}"
    Ok(())
}

#[cfg(not(windows))]
pub fn register_service(_mesh_name: &str, _layer_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows service registration not available on this platform".into())
}