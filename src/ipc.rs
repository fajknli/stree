// src/ipc.rs

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

const MAX_DATA_SIZE: usize = 512 * 1024; // 512KB 硬上限
const IPC_READ_TIMEOUT: Duration = Duration::from_secs(2);
const IPC_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct IpcServer {
    pub socket_path: String,
    listener: UnixListener,
}

impl IpcServer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let pid = std::process::id();
        let socket_path = std::env::var("STREE_SOCK")
            .unwrap_or_else(|_| format!("/tmp/stree_{}.sock", pid));
        // 防线1：启动预删
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        Ok(Self { socket_path, listener })
    }

    pub fn try_accept_and_process<F>(&self, mut handler: F)
    where
        F: FnMut(&str, &str),
    {
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    // 【防御性优化】为已接受的连接设置读写超时，防止恶意或卡死的客户端阻塞主渲染循环
                    let _ = stream.set_read_timeout(Some(IPC_READ_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IPC_WRITE_TIMEOUT));

                    if let Err(e) = handle_connection(&mut stream, &mut handler) {
                        eprintln!("[stree-engine] IPC 处理错误: {}", e);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    eprintln!("[stree-engine] IPC Accept 错误: {}", e);
                    break;
                }
            }
        }
    }
}

fn handle_connection<F>(stream: &mut UnixStream, handler: &mut F) -> std::io::Result<()>
where
    F: FnMut(&str, &str),
{
    let mut header = [0u8; 12];
    stream.read_exact(&mut header)?;

    let target_len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let data_len = u64::from_be_bytes([
        header[4], header[5], header[6], header[7],
        header[8], header[9], header[10], header[11],
    ]) as usize;

    if target_len == 0 || target_len > 128 || data_len > MAX_DATA_SIZE {
        stream.write_all(b"ERROR: Invalid header or data too large")?;
        return Ok(());
    }

    let mut target_buf = vec![0u8; target_len];
    stream.read_exact(&mut target_buf)?;
    let target = String::from_utf8_lossy(&target_buf);

    let mut data_buf = vec![0u8; data_len];
    stream.read_exact(&mut data_buf)?;
    let data = String::from_utf8_lossy(&data_buf);

    handler(&target, &data);

    stream.write_all(b"OK")?;
    Ok(())
}

pub fn run_ctrl_command(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    if target.is_empty() {
        eprintln!("[stree-ctl] Usage: stree update <target_window>");
        std::process::exit(1);
    }

    let socket_path = match std::env::var("STREE_SOCK") {
        Ok(path) => path,
        Err(_) => {
            eprintln!("[stree-ctl] Error: $STREE_SOCK not set. Are you running inside stree?");
            std::process::exit(2);
        }
    };

    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[stree-ctl] Error: Connection refused. Is stree running? ({})", e);
            std::process::exit(3);
        }
    };

    let mut data = String::new();
    // 【安全加固】限制 stdin 读取上限，复用服务端的 MAX_DATA_SIZE 硬限制，防止 OOM
    std::io::stdin().take(MAX_DATA_SIZE as u64).read_to_string(&mut data)?;

    let target_bytes = target.as_bytes();
    let target_len = target_bytes.len() as u32;
    let data_len = data.len() as u64;

    let mut header = [0u8; 12];
    header[0..4].copy_from_slice(&target_len.to_be_bytes());
    header[4..12].copy_from_slice(&data_len.to_be_bytes());

    stream.write_all(&header)?;
    stream.write_all(target_bytes)?;
    stream.write_all(data.as_bytes())?;

    let mut response = [0u8; 2];
    stream.read_exact(&mut response)?;
    if &response != b"OK" {
        eprintln!("[stree-ctl] Error from engine: {:?}", response);
        std::process::exit(4);
    }

    Ok(())
}
