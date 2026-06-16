use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::fs;
use std::path::PathBuf;

// ──────────────────────────────────────────────
//  Windows DPAPI 密钥文件保护
//  DPAPI 将密钥绑定到当前 Windows 用户，防止其他用户读取
// ──────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod dpapi {
    use std::ptr;

    #[repr(C)]
    struct DATA_BLOB {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            pDataIn: *const DATA_BLOB,
            szDataDescr: *const u16,
            pOptionalEntropy: *const DATA_BLOB,
            pvReserved: *mut std::ffi::c_void,
            pPromptStruct: *mut std::ffi::c_void,
            dwFlags: u32,
            pDataOut: *mut DATA_BLOB,
        ) -> i32;

        fn CryptUnprotectData(
            pDataIn: *const DATA_BLOB,
            ppszDataDescr: *mut *mut u16,
            pOptionalEntropy: *const DATA_BLOB,
            pvReserved: *mut std::ffi::c_void,
            pPromptStruct: *mut std::ffi::c_void,
            dwFlags: u32,
            pDataOut: *mut DATA_BLOB,
        ) -> i32;

        fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    /// 使用当前用户凭据加密数据（仅当前用户可解密）
    pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
        let input = DATA_BLOB {
            cb_data: data.len() as u32,
            pb_data: data.as_ptr() as *mut u8,
        };
        let mut output = DATA_BLOB {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };

        let ret = unsafe {
            CryptProtectData(
                &input as *const DATA_BLOB,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut output as *mut DATA_BLOB,
            )
        };

        if ret == 0 {
            return Err("DPAPI 加密失败".to_string());
        }

        let result = unsafe {
            let slice = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize);
            let vec = slice.to_vec();
            LocalFree(output.pb_data as *mut std::ffi::c_void);
            vec
        };

        Ok(result)
    }

    /// 使用当前用户凭据解密数据
    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        let input = DATA_BLOB {
            cb_data: data.len() as u32,
            pb_data: data.as_ptr() as *mut u8,
        };
        let mut output = DATA_BLOB {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };

        let ret = unsafe {
            CryptUnprotectData(
                &input as *const DATA_BLOB,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut output as *mut DATA_BLOB,
            )
        };

        if ret == 0 {
            return Err("DPAPI 解密失败".to_string());
        }

        let result = unsafe {
            let slice = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize);
            let vec = slice.to_vec();
            LocalFree(output.pb_data as *mut std::ffi::c_void);
            vec
        };

        Ok(result)
    }
}

/// 非 Windows 平台回退：直接 base64 编码/解码
#[cfg(not(target_os = "windows"))]
mod dpapi {
    pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
        Ok(data.to_vec())
    }
    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        Ok(data.to_vec())
    }
}

// ──────────────────────────────────────────────
//  密钥管理
// ──────────────────────────────────────────────

/// 获取或创建 AES-256 密钥文件。
/// 密钥文件存储在 app_data_dir/.key，内容通过 DPAPI 加密保护（Windows 平台）。
fn get_or_create_key() -> Result<[u8; 32], String> {
    let key_path = key_file_path();

    if key_path.exists() {
        let raw = fs::read(&key_path).map_err(|e| format!("读取密钥文件失败: {}", e))?;
        // DPAPI 解密
        let decoded = dpapi::unprotect(&raw)?;
        let key_str = String::from_utf8(decoded).map_err(|_| "密钥文件编码无效".to_string())?;
        let key_bytes = BASE64
            .decode(key_str.trim())
            .map_err(|e| format!("解码密钥失败: {}", e))?;
        if key_bytes.len() != 32 {
            return Err("密钥长度无效".to_string());
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&key_bytes);
        return Ok(key);
    }

    // 生成新密钥
    let key = Aes256Gcm::generate_key(OsRng);
    let encoded = BASE64.encode(&key);
    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建密钥目录失败: {}", e))?;
    }
    // DPAPI 加密后写入
    let protected = dpapi::protect(encoded.as_bytes())?;
    fs::write(&key_path, &protected).map_err(|e| format!("写入密钥文件失败: {}", e))?;

    log::info!("[crypto] AES-256 key generated and DPAPI-protected at {:?}", key_path);
    Ok(key.into())
}

fn key_file_path() -> PathBuf {
    crate::utils::paths::app_data_dir().join(".key")
}

// ──────────────────────────────────────────────
//  公开加密/解密接口
// ──────────────────────────────────────────────

/// 使用 AES-256-GCM 加密明文。
/// 返回 base64(nonce || ciphertext)。
pub fn encrypt(plaintext: &str) -> Result<String, String> {
    let key = get_or_create_key()?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("创建 cipher 失败: {}", e))?;

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| format!("加密失败: {}", e))?;

    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(&combined))
}

/// 解密 base64(nonce || ciphertext)。
pub fn decrypt(encoded: &str) -> Result<String, String> {
    let key = get_or_create_key()?;
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("创建 cipher 失败: {}", e))?;

    let combined = BASE64
        .decode(encoded)
        .map_err(|e| format!("解码失败: {}", e))?;
    if combined.len() < 12 {
        return Err("数据长度不足".to_string());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("解密失败: {}", e))?;

    String::from_utf8(plaintext).map_err(|e| format!("UTF-8 解码失败: {}", e))
}
