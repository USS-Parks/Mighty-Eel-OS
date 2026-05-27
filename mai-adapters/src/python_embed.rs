use pyo3::prelude::*;

#[derive(Debug, Clone)]
pub struct PythonRuntimeInfo {
    pub executable: String,
    pub version: String,
}

pub fn python_runtime_info() -> Option<PythonRuntimeInfo> {
    Python::with_gil(|py| {
        let sys = py.import("sys").ok()?;
        let executable: String = sys.getattr("executable").ok()?.extract().ok()?;
        let version: String = sys.getattr("version").ok()?.extract().ok()?;
        Some(PythonRuntimeInfo {
            executable,
            version,
        })
    })
}
