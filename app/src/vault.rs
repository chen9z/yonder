//! What the phone keeps secret (its SSH key, a password not yet replaced
//! by it), sealed by the Android Keystore: an AES key that never leaves the
//! phone encrypts it, so a backup or a copied app folder holds only what
//! cannot be opened anywhere else. Before Android 12 (which doesn't let an
//! app reach its JVM this way) the key is the app's own, in its
//! `no_backup` folder: backups and transfers leave it out. Off a phone
//! (tests), the words stay as they are.

/// Sealed words start so; what doesn't is kept as it was written.
const SEALED: &str = "sealed1:";
/// Sealed with the app's own key.
const ASIDE: &str = "aside1:";

/// Encryption failures are errors on a phone. Desktop headless tests have
/// an explicit plaintext backend and never exercise Android persistence.
pub fn seal(plain: &str) -> Result<String, String> {
    let sealed = seal_with(plain, keystore::seal(plain.as_bytes()), || aside::folder().ok_or_else(|| "no app folder".into()));
    #[cfg(target_os = "android")]
    { sealed }
    #[cfg(not(target_os = "android"))]
    { sealed.or_else(|_| Ok(plain.to_owned())) }
}

fn seal_with(plain: &str, primary: Result<(Vec<u8>, Vec<u8>), String>, folder: impl FnOnce() -> Result<std::path::PathBuf, String>) -> Result<String, String> {
    primary
        .map(|(iv, sealed)| format!("{SEALED}{}:{}", hex(&iv), hex(&sealed)))
        .or_else(|why| {
            log::info!("no Keystore ({why}): the app's own key seals");
            let dir = folder()?;
            aside::seal(&dir, plain.as_bytes()).map(|(iv, sealed)| format!("{ASIDE}{}:{}", hex(&iv), hex(&sealed)))
        })
}

/// What `kept` holds: opened if sealed (None if it can't be: sealed on
/// another phone, or before the app was installed again), else as it is.
pub fn open(kept: &str) -> Option<String> {
    let opened = if let Some(sealed) = kept.strip_prefix(SEALED) {
        let (iv, sealed) = sealed.split_once(':')?;
        keystore::open(&unhex(iv)?, &unhex(sealed)?)
    } else if let Some(sealed) = kept.strip_prefix(ASIDE) {
        let (iv, sealed) = sealed.split_once(':')?;
        let (iv, sealed) = (unhex(iv)?, unhex(sealed)?);
        aside::folder().ok_or_else(|| "no app folder".to_owned()).and_then(|dir| aside::open(&dir, &iv, &sealed))
    } else {
        return Some(kept.to_owned());
    };
    match opened {
        Ok(plain) => String::from_utf8(plain).ok(),
        Err(why) => {
            log::warn!("can't open what was sealed: {why}");
            None
        }
    }
}

/// Whether `kept` is sealed.
#[cfg(test)]
fn sealed(kept: &str) -> bool {
    kept.starts_with(SEALED) || kept.starts_with(ASIDE)
}

/// How `kept` is sealed, said for a diagnosis.
pub fn how(kept: &str) -> &'static str {
    if kept.starts_with(SEALED) {
        "Android Keystore"
    } else if kept.starts_with(ASIDE) {
        "the app's own key"
    } else {
        "unsealed"
    }
}

/// The app's own key, for phones without a Keystore an app can reach: 32
/// random bytes in a file only the app reads, outside its backups.
mod aside {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
    use ring::rand::{SecureRandom, SystemRandom};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    const FILE: &str = "herdc.vault";

    /// The app's `no_backup` folder.
    pub fn folder() -> Option<PathBuf> {
        crate::app_folder("no_backup")
    }

    fn key(dir: &Path) -> Result<LessSafeKey, String> {
        let path = dir.join(FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = vec![0u8; 32];
                SystemRandom::new().fill(&mut bytes).map_err(|_| "no randomness")?;
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                let mut file = owner_only(&path).map_err(|e| e.to_string())?;
                file.write_all(&bytes).map_err(|e| e.to_string())?;
                bytes
            }
            Err(e) => return Err(e.to_string()),
        };
        Ok(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &bytes).map_err(|_| "a broken key file")?))
    }

    #[cfg(unix)]
    fn owner_only(path: &Path) -> std::io::Result<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
    }

    #[cfg(not(unix))]
    fn owner_only(path: &Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().write(true).create_new(true).open(path)
    }

    /// (nonce, sealed)
    pub fn seal(dir: &Path, plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let key = key(dir)?;
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| "no randomness")?;
        let mut sealed = plain.to_vec();
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut sealed).map_err(|_| "not sealed")?;
        Ok((nonce.to_vec(), sealed))
    }

    pub fn open(dir: &Path, nonce: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
        let key = key(dir)?;
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| "a broken nonce")?;
        let mut sealed = sealed.to_vec();
        let plain = key.open_in_place(nonce, Aad::empty(), &mut sealed).map_err(|_| "sealed with another key")?;
        Ok(plain.to_vec())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    text.len().is_multiple_of(2).then_some(())?;
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(not(target_os = "android"))]
mod keystore {
    pub fn seal(_: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        Err("no Keystore here".into())
    }

    pub fn open(_: &[u8], _: &[u8]) -> Result<Vec<u8>, String> {
        Err("no Keystore here".into())
    }
}

/// The Keystore through JNI: `javax.crypto` with an `AndroidKeyStore` key,
/// AES-GCM, as an app's Java would.
#[cfg(target_os = "android")]
mod keystore {
    use jni::objects::{JByteArray, JObject, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::sync::OnceLock;

    const ALIAS: &str = "herdc.vault";
    const ENCRYPT: i32 = 1;
    const DECRYPT: i32 = 2;

    /// The app's JVM: `JNI_GetCreatedJavaVMs`, public in libnativehelper
    /// from Android 12.
    fn vm() -> Result<&'static JavaVM, String> {
        static VM: OnceLock<Option<JavaVM>> = OnceLock::new();
        let vm = VM.get_or_init(|| unsafe {
            type Created = unsafe extern "system" fn(*mut *mut jni::sys::JavaVM, jni::sys::jsize, *mut jni::sys::jsize) -> jni::sys::jint;
            let lib = libc::dlopen(c"libnativehelper.so".as_ptr(), libc::RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            let created = libc::dlsym(lib, c"JNI_GetCreatedJavaVMs".as_ptr());
            if created.is_null() {
                return None;
            }
            let created: Created = std::mem::transmute(created);
            let (mut vm, mut count) = (std::ptr::null_mut(), 0);
            if created(&mut vm, 1, &mut count) != 0 || count < 1 {
                return None;
            }
            JavaVM::from_raw(vm).ok()
        });
        vm.as_ref().ok_or_else(|| "no JVM to reach (Android before 12)".to_owned())
    }

    /// Runs `f` on this thread's JNIEnv; a Java exception is cleared and said.
    fn with<T>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<T>) -> Result<T, String> {
        let mut env = vm()?.attach_current_thread().map_err(|e| e.to_string())?;
        let done = f(&mut env);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        done.map_err(|e| e.to_string())
    }

    /// The app's AES key in the Keystore, made the first time.
    fn key<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
        let provider = env.new_string("AndroidKeyStore")?;
        let alias = env.new_string(ALIAS)?;
        let store = env
            .call_static_method("java/security/KeyStore", "getInstance", "(Ljava/lang/String;)Ljava/security/KeyStore;", &[JValue::Object(&provider)])?
            .l()?;
        env.call_method(&store, "load", "(Ljava/security/KeyStore$LoadStoreParameter;)V", &[JValue::Object(&JObject::null())])?;
        if !env.call_method(&store, "containsAlias", "(Ljava/lang/String;)Z", &[JValue::Object(&alias)])?.z()? {
            let builder = env.new_object(
                "android/security/keystore/KeyGenParameterSpec$Builder",
                "(Ljava/lang/String;I)V",
                &[JValue::Object(&alias), JValue::Int(ENCRYPT | DECRYPT)],
            )?;
            let words = |env: &mut JNIEnv<'a>, word: &str| -> jni::errors::Result<JObject<'a>> {
                let word = env.new_string(word)?;
                Ok(env.new_object_array(1, "java/lang/String", word)?.into())
            };
            const BUILDER: &str = "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;";
            let gcm = words(env, "GCM")?;
            env.call_method(&builder, "setBlockModes", BUILDER, &[JValue::Object(&gcm)])?;
            let none = words(env, "NoPadding")?;
            env.call_method(&builder, "setEncryptionPaddings", BUILDER, &[JValue::Object(&none)])?;
            let spec = env.call_method(&builder, "build", "()Landroid/security/keystore/KeyGenParameterSpec;", &[])?.l()?;
            let aes = env.new_string("AES")?;
            let generator = env
                .call_static_method(
                    "javax/crypto/KeyGenerator",
                    "getInstance",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
                    &[JValue::Object(&aes), JValue::Object(&provider)],
                )?
                .l()?;
            env.call_method(&generator, "init", "(Ljava/security/spec/AlgorithmParameterSpec;)V", &[JValue::Object(&spec)])?;
            env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[])?;
        }
        env.call_method(&store, "getKey", "(Ljava/lang/String;[C)Ljava/security/Key;", &[JValue::Object(&alias), JValue::Object(&JObject::null())])?.l()
    }

    fn cipher<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
        let name = env.new_string("AES/GCM/NoPadding")?;
        env.call_static_method("javax/crypto/Cipher", "getInstance", "(Ljava/lang/String;)Ljavax/crypto/Cipher;", &[JValue::Object(&name)])?.l()
    }

    fn finish(env: &mut JNIEnv, cipher: &JObject, input: &[u8]) -> jni::errors::Result<Vec<u8>> {
        let input = env.byte_array_from_slice(input)?;
        let output: JByteArray = env.call_method(cipher, "doFinal", "([B)[B", &[JValue::Object(&input)])?.l()?.into();
        env.convert_byte_array(output)
    }

    /// (iv, sealed)
    pub fn seal(plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        with(|env| {
            let (key, cipher) = (key(env)?, cipher(env)?);
            env.call_method(&cipher, "init", "(ILjava/security/Key;)V", &[JValue::Int(ENCRYPT), JValue::Object(&key)])?;
            let iv: JByteArray = env.call_method(&cipher, "getIV", "()[B", &[])?.l()?.into();
            let iv = env.convert_byte_array(iv)?;
            Ok((iv, finish(env, &cipher, plain)?))
        })
    }

    pub fn open(iv: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
        with(|env| {
            let (key, cipher) = (key(env)?, cipher(env)?);
            let iv = env.byte_array_from_slice(iv)?;
            let spec = env.new_object("javax/crypto/spec/GCMParameterSpec", "(I[B)V", &[JValue::Int(128), JValue::Object(&iv)])?;
            env.call_method(
                &cipher,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Int(DECRYPT), JValue::Object(&key), JValue::Object(&spec)],
            )?;
            finish(env, &cipher, sealed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_goes_both_ways() {
        let bytes = [0u8, 1, 0xab, 0xff];
        assert_eq!(unhex(&hex(&bytes)).as_deref(), Some(&bytes[..]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn without_a_keystore_words_stay_as_they_are() {
        assert_eq!(seal("secret").unwrap(), "secret");
        assert_eq!(open("secret").as_deref(), Some("secret"));
        assert!(!sealed("secret"));
        // Sealed elsewhere: not opened here, and not taken for the words.
        assert_eq!(open("sealed1:00:00"), None);
    }

    #[test]
    fn failed_encryption_never_returns_plaintext() {
        let dir = std::env::temp_dir().join(format!("herdc-broken-vault-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("herdc.vault"), b"broken").unwrap();
        assert!(seal_with("synthetic", Err("no keystore".into()), || Ok(dir.clone())).is_err());
        assert!(seal_with("synthetic", Err("no keystore".into()), || Err("no folder".into())).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_apps_own_key_opens_what_it_sealed_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("herdc-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (nonce, sealed) = aside::seal(&dir, b"a private key").unwrap();
        assert!(!sealed.windows(7).any(|w| w == b"private"));
        assert_eq!(aside::open(&dir, &nonce, &sealed).unwrap(), b"a private key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("herdc.vault")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Another key (the app installed again, or another phone): not opened.
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(aside::open(&dir, &nonce, &sealed).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
