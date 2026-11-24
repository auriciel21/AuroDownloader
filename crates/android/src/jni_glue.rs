//! Liaison JNI avec la classe Kotlin `org.aurodownloader.engine.NativeEngine` :
//!
//! - `start(context: Context, db: String): String` : initialise la vérification des
//!   certificats d'Android (HTTPS), puis démarre le moteur sur la base `db` ; rend `""`,
//!   ou la raison d'un échec ;
//! - `send(message: String)` : un message JSON-RPC pour le moteur ;
//! - `stop()` : arrête le moteur, ses tâches enregistrées ;
//! - en retour, le moteur appelle `NativeEngine.onMessage(message: String)` (statique),
//!   depuis l'un de ses fils, pour chaque réponse et chaque notification.

use std::path::Path;
use std::sync::Mutex;

use jni::errors::{Error, ThrowRuntimeExAndDefault};
use jni::objects::{JClass, JObject, JString, JValue};
use jni::{EnvUnowned, jni_sig, jni_str};

use crate::Host;

/// Le moteur du processus : un seul, démarré par `start`.
static HOST: Mutex<Option<Host>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_aurodownloader_engine_NativeEngine_start<'caller>(
    mut unowned: EnvUnowned<'caller>,
    class: JClass<'caller>,
    context: JObject<'caller>,
    db: JString<'caller>,
) -> JString<'caller> {
    unowned
        .with_env(|env| -> Result<JString<'caller>, Error> {
            let mut host = HOST.lock().unwrap_or_else(|e| e.into_inner());
            if host.is_some() {
                return env.new_string("");
            }
            // Avant toute requête : sans elle, le vérificateur panique au premier
            // certificat (SPEC §7.5).
            rustls_platform_verifier::android::init_with_env(env, context)?;
            auro_engine::net::platform_tls_ready();
            let db = db.try_to_string(env)?;
            let vm = env.get_java_vm()?;
            let class = env.new_global_ref(class)?;
            let on_message = move |line: String| {
                let _ = vm.attach_current_thread(|env| -> Result<(), Error> {
                    let message = env.new_string(line)?;
                    env.call_static_method(
                        &class,
                        jni_str!("onMessage"),
                        jni_sig!("(Ljava/lang/String;)V"),
                        &[JValue::Object(&message)],
                    )?;
                    Ok(())
                });
            };
            match Host::start(Path::new(&db), on_message) {
                Ok(started) => {
                    *host = Some(started);
                    env.new_string("")
                }
                Err(reason) => env.new_string(reason),
            }
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_aurodownloader_engine_NativeEngine_send<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    message: JString<'caller>,
) {
    unowned
        .with_env(|env| -> Result<(), Error> {
            let message = message.try_to_string(env)?;
            if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                host.send(&message);
            }
            Ok(())
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}
