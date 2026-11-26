//! What the phone keeps secret (its SSH key, a password not yet replaced
//! by it), sealed by the Android Keystore: an AES key that never leaves the
//! phone encrypts it, so a backup or a copied app folder holds only what
//! cannot be opened anywhere else. Before Android 12 (which doesn't let an
//! app reach its JVM this way) the key is the app's own, in its
//! `no_backup` folder: backups and transfers leave it out. On a Mac the
//! key is in the login keychain. In tests (the unit tests, and the
//! headless ones with `test-vault`) the key is the test process's own, in
//! its memory only. Whatever the build, sealed words are never the words.

/// Sealed words start so; what doesn't is kept as it was written.
const SEALED: &str = "sealed1:";
/// Sealed with the app's own key.
const ASIDE: &str = "aside1:";
/// Sealed with the key in the Mac's login keychain.
const KEYCHAIN: &str = "keychain1:";

#[cfg(all(feature = "test-vault", not(debug_assertions)))]
compile_error!("test-vault keeps the key in memory: for the headless tests only, never a release");

/// Encryption failures are errors: nothing is ever kept as it is.
pub fn seal(plain: &str) -> Result<String, String> {
    #[cfg(target_os = "android")]
    { seal_with(plain, keystore::seal(plain.as_bytes()), || aside::folder().ok_or_else(|| "no app folder".into())) }
    #[cfg(all(not(target_os = "android"), any(test, feature = "test-vault")))]
    { seal_by(&aside::test_key()?, plain.as_bytes()).map(|(iv, sealed)| format!("{ASIDE}{}:{}", hex(&iv), hex(&sealed))) }
    #[cfg(all(target_os = "macos", not(any(test, feature = "test-vault"))))]
    { keychain::LOGIN.seal(plain.as_bytes()) }
    #[cfg(all(not(any(target_os = "android", target_os = "macos")), not(any(test, feature = "test-vault"))))]
    { Err("nothing seals here".into()) }
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
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
        #[cfg(all(not(target_os = "android"), any(test, feature = "test-vault")))]
        { aside::test_key().and_then(|key| open_by(&key, &iv, &sealed)) }
        #[cfg(not(all(not(target_os = "android"), any(test, feature = "test-vault"))))]
        { aside::folder().ok_or_else(|| "no app folder".to_owned()).and_then(|dir| aside::open(&dir, &iv, &sealed)) }
    } else if let Some(sealed) = kept.strip_prefix(KEYCHAIN) {
        let (iv, sealed) = sealed.split_once(':')?;
        let (iv, sealed) = (unhex(iv)?, unhex(sealed)?);
        // Tests never reach the person's keychain.
        #[cfg(all(target_os = "macos", not(any(test, feature = "test-vault"))))]
        { keychain::LOGIN.open(&iv, &sealed) }
        #[cfg(not(all(target_os = "macos", not(any(test, feature = "test-vault")))))]
        { let _ = (iv, sealed); Err("no keychain here".to_owned()) }
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
    kept.starts_with(SEALED) || kept.starts_with(ASIDE) || kept.starts_with(KEYCHAIN)
}

/// How `kept` is sealed, said for a diagnosis.
pub fn how(kept: &str) -> &'static str {
    if kept.starts_with(SEALED) {
        "Android Keystore"
    } else if kept.starts_with(ASIDE) {
        "the app's own key"
    } else if kept.starts_with(KEYCHAIN) {
        "macOS Keychain"
    } else {
        "unsealed"
    }
}

/// The app's own key, for phones without a Keystore an app can reach: 32
/// random bytes in a file only the app reads, outside its backups.
mod aside {
    use ring::aead::{LessSafeKey, UnboundKey, AES_256_GCM};
    use ring::rand::{SecureRandom, SystemRandom};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    const FILE: &str = "herdc.vault";

    /// The app's `no_backup` folder.
    #[cfg_attr(all(not(target_os = "android"), any(test, feature = "test-vault")), allow(dead_code))]
    pub fn folder() -> Option<PathBuf> {
        crate::app_folder("no_backup")
    }

    fn key(dir: &Path) -> Result<LessSafeKey, String> {
        let path = dir.join(FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let bytes = random_key()?;
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                let mut file = owner_only(&path).map_err(|e| e.to_string())?;
                file.write_all(&bytes).map_err(|e| e.to_string())?;
                bytes
            }
            Err(e) => return Err(e.to_string()),
        };
        aes(&bytes).map_err(|_| "a broken key file".into())
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
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub fn seal(dir: &Path, plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
        super::seal_by(&key(dir)?, plain)
    }

    #[cfg_attr(all(not(target_os = "android"), feature = "test-vault", not(test)), allow(dead_code))]
    pub fn open(dir: &Path, nonce: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
        super::open_by(&key(dir)?, nonce, sealed)
    }

    /// An AES-256-GCM key of `bytes`.
    pub fn aes(bytes: &[u8]) -> Result<LessSafeKey, String> {
        Ok(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| "a broken key")?))
    }

    /// 32 random bytes, for a new key.
    pub fn random_key() -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; 32];
        SystemRandom::new().fill(&mut bytes).map_err(|_| "no randomness")?;
        Ok(bytes)
    }

    /// The tests' key off a phone: the process's own, made the first time,
    /// in its memory only (never on disk, gone when the tests end).
    #[cfg(all(not(target_os = "android"), any(test, feature = "test-vault")))]
    pub fn test_key() -> Result<LessSafeKey, String> {
        static KEY: std::sync::OnceLock<Result<Vec<u8>, String>> = std::sync::OnceLock::new();
        aes(KEY.get_or_init(random_key).as_ref().map_err(Clone::clone)?)
    }
}

/// (nonce, sealed) of `plain` by `key`, a new random nonce each time.
#[cfg_attr(not(any(target_os = "android", target_os = "macos", test)), allow(dead_code))]
fn seal_by(key: &ring::aead::LessSafeKey, plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    use ring::aead::{Aad, Nonce, NONCE_LEN};
    use ring::rand::{SecureRandom, SystemRandom};
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(|_| "no randomness")?;
    let mut sealed = plain.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut sealed).map_err(|_| "not sealed")?;
    Ok((nonce.to_vec(), sealed))
}

fn open_by(key: &ring::aead::LessSafeKey, nonce: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
    use ring::aead::{Aad, Nonce};
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| "a broken nonce")?;
    let mut sealed = sealed.to_vec();
    let plain = key.open_in_place(nonce, Aad::empty(), &mut sealed).map_err(|_| "sealed with another key")?;
    Ok(plain.to_vec())
}

/// The key on a Mac: 32 random bytes in the login keychain (service
/// [`keychain::LOGIN`]'s), made the first time, read and written by
/// `/usr/bin/security`, in the login keychain by its path both ways. The
/// key goes to it on its stdin, never in its arguments. No keychain, no
/// sealing: the words are never kept as they are.
#[cfg(target_os = "macos")]
#[cfg_attr(any(test, feature = "test-vault"), allow(dead_code))]
mod keychain {
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// An item of the login keychain, and the tool that reaches it.
    pub struct Keychain {
        pub service: &'static str,
        pub tool: &'static str,
    }

    pub const LOGIN: Keychain = Keychain { service: "dev.yonder.app.vault", tool: "/usr/bin/security" };
    const ACCOUNT: &str = "key";
    /// `security`'s exit status for an item that isn't there.
    const NOT_FOUND: i32 = 44;

    impl Keychain {
        pub fn seal(&self, plain: &[u8]) -> Result<String, String> {
            let (iv, sealed) = super::seal_by(&self.key()?, plain)?;
            Ok(format!("{}{}:{}", super::KEYCHAIN, super::hex(&iv), super::hex(&sealed)))
        }

        pub fn open(&self, iv: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
            super::open_by(&self.key()?, iv, sealed)
        }

        fn key(&self) -> Result<ring::aead::LessSafeKey, String> {
            let words = match self.find()? {
                Some(words) => words,
                None => {
                    // Made at the same time elsewhere: that one is kept, and read.
                    let made = self.add(&super::hex(&super::aside::random_key()?));
                    self.find()?.ok_or_else(|| made.err().unwrap_or_else(|| "not kept".into()))?
                }
            };
            let bytes = super::unhex(words.trim()).filter(|b| b.len() == 32).ok_or("a broken key in the keychain")?;
            super::aside::aes(&bytes)
        }

        /// The login keychain's file, as `security login-keychain` says it.
        pub fn login_keychain(&self) -> Result<String, String> {
            let out = Command::new(self.tool).arg("login-keychain").stdin(Stdio::null()).output().map_err(|e| format!("{}: {e}", self.tool))?;
            let path = String::from_utf8_lossy(&out.stdout).trim().trim_matches('"').to_owned();
            if !out.status.success() || path.is_empty() || path.contains('"') {
                return Err(format!("no login keychain: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            Ok(path)
        }

        fn find(&self) -> Result<Option<String>, String> {
            let login = self.login_keychain()?;
            let out = Command::new(self.tool)
                .args(["find-generic-password", "-s", self.service, "-a", ACCOUNT, "-w", &login])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("{}: {e}", self.tool))?;
            match out.status.code() {
                Some(0) => String::from_utf8(out.stdout).map(Some).map_err(|_| "a broken key in the keychain".into()),
                Some(NOT_FOUND) => Ok(None),
                _ => Err(format!("no keychain: {}", String::from_utf8_lossy(&out.stderr).trim())),
            }
        }

        fn add(&self, words: &str) -> Result<(), String> {
            let login = self.login_keychain()?;
            let mut child = Command::new(self.tool)
                .arg("-i")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("{}: {e}", self.tool))?;
            let line = format!("add-generic-password -s {} -a {ACCOUNT} -w {words} \"{login}\"\n", self.service);
            let wrote = child.stdin.take().ok_or("no stdin")?.write_all(line.as_bytes());
            let out = child.wait_with_output().map_err(|e| e.to_string())?;
            wrote.map_err(|e| e.to_string())?;
            if out.status.success() { Ok(()) } else { Err(format!("not kept in the keychain: {}", String::from_utf8_lossy(&out.stderr).trim())) }
        }
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
    fn without_a_keystore_the_tests_key_seals() {
        let kept = seal("secret").unwrap();
        assert!(kept.starts_with(ASIDE) && !kept.contains("secret") && !kept.contains(&hex(b"secret")), "{kept}");
        assert_eq!(open(&kept).as_deref(), Some("secret"));
        assert_ne!(seal("secret").unwrap(), kept);
        // Kept unsealed by an earlier build: read as it is.
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

    /// The test item of the login keychain, gone before and after.
    #[cfg(target_os = "macos")]
    struct TestItem;

    #[cfg(target_os = "macos")]
    const TEST: keychain::Keychain = keychain::Keychain { service: "dev.yonder.app.vault.test", tool: "/usr/bin/security" };

    #[cfg(target_os = "macos")]
    impl TestItem {
        fn new() -> Self {
            Self::remove();
            TestItem
        }

        fn remove() {
            let login = TEST.login_keychain().unwrap();
            let _ = std::process::Command::new(TEST.tool).args(["delete-generic-password", "-s", TEST.service, &login]).output();
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for TestItem {
        fn drop(&mut self) {
            Self::remove();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_round_trip() {
        let _item = TestItem::new();
        let kept = TEST.seal(b"a private key").unwrap();
        assert!(kept.starts_with(KEYCHAIN), "{kept}");
        assert!(sealed(&kept));
        assert_eq!(how(&kept), "macOS Keychain");
        assert!(!kept.contains("a private key") && !kept.contains(&hex(b"a private key")), "{kept}");
        // A new nonce each time, the same key from the keychain.
        let again = TEST.seal(b"a private key").unwrap();
        assert_ne!(kept, again);
        for kept in [kept, again] {
            let (iv, sealed) = kept.strip_prefix(KEYCHAIN).unwrap().split_once(':').unwrap();
            assert_eq!(TEST.open(&unhex(iv).unwrap(), &unhex(sealed).unwrap()).unwrap(), b"a private key");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn never_seals_to_plaintext() {
        // No keychain to reach, or one that refuses: an error, never the words.
        for tool in ["/nonexistent/security", "/usr/bin/false"] {
            let gone = keychain::Keychain { service: "dev.yonder.app.vault.test", tool };
            assert!(gone.seal(b"synthetic").is_err(), "{tool}");
            assert!(gone.open(&[0; 12], &[0; 32]).is_err(), "{tool}");
        }
        // The tests' own keychain1 words never open from the person's keychain.
        assert_eq!(open(&format!("{KEYCHAIN}00:00")), None);
    }

    /// The headless tests' build: the public seal still never keeps the words.
    #[cfg(feature = "test-vault")]
    #[test]
    fn test_vault_seals_too() {
        for words in ["open sesame", "-----BEGIN OPENSSH PRIVATE KEY-----"] {
            let kept = seal(words).unwrap();
            assert!(kept.starts_with(ASIDE) || kept.starts_with(KEYCHAIN), "{kept}");
            assert!(!kept.contains(words) && !kept.contains(&hex(words.as_bytes())), "{kept}");
            assert_eq!(open(&kept).as_deref(), Some(words));
        }
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
