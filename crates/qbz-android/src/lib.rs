use std::collections::HashMap;
use std::io::Write as IoWrite;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use jni::objects::{JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;
use qbz_models::{Quality, Track};
use qbz_qobuz::QobuzClient;
use qbz_usb_direct::{AndroidUsbDirectConfig, AndroidUsbDirectStream, PcmFormat};
use serde_json::json;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::default::{get_codecs, get_probe};

struct CursorSource(Cursor<Vec<u8>>);
impl Read for CursorSource {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Seek for CursorSource {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(p)
    }
}
impl MediaSource for CursorSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.0.get_ref().len() as u64)
    }
}

#[derive(Clone)]
struct Prepared {
    bytes: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    bit_depth: u8,
    track_id: u64,
}
struct OAuthPending {
    listener: TcpListener,
    nonce: String,
}
struct State {
    runtime: tokio::runtime::Runtime,
    client: QobuzClient,
    prepared: HashMap<u64, Prepared>,
    oauth: Option<OAuthPending>,
    data_dir: PathBuf,
}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static PAUSE_REQUESTED: AtomicBool = AtomicBool::new(false);

fn result_json(result: Result<serde_json::Value, String>) -> String {
    match result {
        Ok(value) => json!({"ok":true,"data":value}).to_string(),
        Err(error) => json!({"ok":false,"error":error}).to_string(),
    }
}

fn to_rust(env: &mut JNIEnv, value: JString) -> Result<String, String> {
    env.get_string(&value)
        .map(|s| s.into())
        .map_err(|e| e.to_string())
}

fn to_java(env: &mut JNIEnv, value: String) -> jstring {
    env.new_string(value)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn with_state<T>(f: impl FnOnce(&mut State) -> Result<T, String>) -> Result<T, String> {
    let mutex = STATE.get().ok_or("QBZ native core is not initialized")?;
    let mut state = mutex.lock().map_err(|_| "QBZ native state is poisoned")?;
    f(&mut state)
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_initialize(
    mut env: JNIEnv,
    _class: JClass,
    context: JObject,
    cache_dir: JString,
) -> jstring {
    let reply = (|| {
        if let Some(mutex) = STATE.get() {
            let state = mutex
                .lock()
                .map_err(|_| "QBZ native state is poisoned".to_string())?;
            let has_session = state.runtime.block_on(state.client.has_session());
            return Ok(json!({"ready":true,"hasSession":has_session}));
        }
        let data_dir = PathBuf::from(to_rust(&mut env, cache_dir)?);
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let raw_env = env.get_raw() as *mut jni22::sys::JNIEnv;
        let raw_context = context.as_raw() as jni22::sys::jobject;
        let mut hosted_env = unsafe { jni22::EnvUnowned::from_raw(raw_env) };
        match hosted_env
            .with_env(|env22| -> jni22::errors::Result<()> {
                let context22 = unsafe { jni22::objects::JObject::from_raw(env22, raw_context) };
                rustls_platform_verifier::android::init_with_env(env22, context22)
            })
            .into_outcome()
        {
            jni22::Outcome::Ok(()) => {}
            jni22::Outcome::Err(error) => {
                return Err(format!(
                    "Android TLS verifier initialization failed: {error}"
                ))
            }
            jni22::Outcome::Panic(_) => {
                return Err("Android TLS verifier initialization panicked".to_string())
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let client = QobuzClient::with_cache_dir(data_dir.clone()).map_err(|e| e.to_string())?;
        runtime.block_on(client.init()).map_err(|e| e.to_string())?;
        let has_session =
            match qbz_credentials::load_oauth_token_at(&data_dir).map_err(|e| e.to_string())? {
                Some(token) => runtime.block_on(client.login_with_token(&token)).is_ok(),
                None => false,
            };
        STATE
            .set(Mutex::new(State {
                runtime,
                client,
                prepared: HashMap::new(),
                oauth: None,
                data_dir,
            }))
            .map_err(|_| "native state already initialized".to_string())?;
        Ok(json!({"ready":true,"hasSession":has_session}))
    })();
    to_java(&mut env, result_json(reply))
}

fn gen_nonce() -> String {
    use rand::RngExt;
    use std::fmt::Write as FmtWrite;
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    let mut value = String::with_capacity(48);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key)
            .then(|| urlencoding::decode(value).ok().map(|v| v.into_owned()))
            .flatten()
    })
}

fn parse_callback(request_line: &str, expected_nonce: &str) -> Option<String> {
    let target = request_line.split_whitespace().nth(1)?;
    let (path, query) = target.split_once('?')?;
    if path.trim_matches('/') != expected_nonce {
        return None;
    }
    query_param(query, "code_autorisation").or_else(|| query_param(query, "code"))
}

fn capture_oauth_code(listener: TcpListener, expected_nonce: &str) -> Result<String, String> {
    listener.set_nonblocking(false).map_err(|e| e.to_string())?;
    for incoming in listener.incoming() {
        let mut stream = incoming.map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 8192];
        let count = stream.read(&mut buffer).map_err(|e| e.to_string())?;
        let request = String::from_utf8_lossy(&buffer[..count]);
        let code = parse_callback(request.lines().next().unwrap_or(""), expected_nonce);
        let body = if code.is_some() {
            "<html><meta name=viewport content='width=device-width'><body style='font-family:system-ui;text-align:center;padding:48px;background:#101513;color:white'><h2>Qobuz login complete</h2><p>QBZ USB Directに戻ってください。</p></body></html>"
        } else {
            "<html><body style='font-family:system-ui;background:#101513;color:white'><p>Qobuzからの認証を待っています。</p></body></html>"
        };
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        if let Some(code) = code {
            return Ok(code);
        }
    }
    Err("Qobuz OAuth callback was not received".to_string())
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_beginOAuth(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let reply = with_state(|state| {
        let app_id = state
            .runtime
            .block_on(state.client.app_id())
            .map_err(|e| e.to_string())?;
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("OAuth listener: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let nonce = gen_nonce();
        let redirect = format!("http://127.0.0.1:{port}/{nonce}");
        let url = format!(
            "https://www.qobuz.com/signin/oauth?ext_app_id={}&redirect_url={}",
            app_id,
            urlencoding::encode(&redirect)
        );
        state.oauth = Some(OAuthPending { listener, nonce });
        Ok(json!({"url":url}))
    });
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_finishOAuth(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let reply = (|| {
        let (pending, client, handle, data_dir) = with_state(|state| {
            let pending = state.oauth.take().ok_or("OAuth login was not started")?;
            Ok((
                pending,
                state.client.clone(),
                state.runtime.handle().clone(),
                state.data_dir.clone(),
            ))
        })?;
        let code = capture_oauth_code(pending.listener, &pending.nonce)?;
        let session = match handle.block_on(client.login_with_oauth_code(&code)) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[finishOAuth] login_with_oauth_code error: {:?}", e);
                return Err(e.to_string());
            }
        };
        qbz_credentials::save_oauth_token_at(&data_dir, &session.user_auth_token)
            .map_err(|e| format!("ログイン情報の保存に失敗しました: {e}"))?;
        Ok(json!({"displayName":session.display_name,"subscription":session.subscription_label}))
    })();
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_login(
    mut env: JNIEnv,
    _class: JClass,
    email: JString,
    password: JString,
) -> jstring {
    let reply = (|| {
        let email = to_rust(&mut env, email)?;
        let password = to_rust(&mut env, password)?;
        with_state(|state| {
            let session = state
                .runtime
                .block_on(state.client.login(&email, &password))
                .map_err(|e| e.to_string())?;
            Ok(
                json!({"displayName":session.display_name,"subscription":session.subscription_label}),
            )
        })
    })();
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_search(
    mut env: JNIEnv,
    _class: JClass,
    query: JString,
) -> jstring {
    let reply = (|| {
        let query = to_rust(&mut env, query)?;
        with_state(|state| {
            let tracks = state
                .runtime
                .block_on(state.client.search_tracks(&query, 20, 0, None))
                .map_err(|e| e.to_string())?;
            let albums = state
                .runtime
                .block_on(state.client.search_albums(&query, 8, 0, None))
                .map_err(|e| e.to_string())?;
            let artists = state
                .runtime
                .block_on(state.client.search_artists(&query, 8, 0, None))
                .map_err(|e| e.to_string())?;
            Ok(json!({
                "tracks": tracks.items.into_iter().map(track_json).collect::<Vec<_>>(),
                "albums": albums.items.into_iter().map(|album| json!({
                    "id":album.id,"title":album.title,"artist":album.artist.name,
                    "artwork":album.image.for_px(300).cloned().unwrap_or_default(),
                    "tracks":album.tracks_count.or(album.track_count).unwrap_or(0),
                    "rate":album.maximum_sampling_rate,"bits":album.maximum_bit_depth
                })).collect::<Vec<_>>(),
                "artists": artists.items.into_iter().map(|artist| json!({
                    "id":artist.id,"name":artist.name,
                    "artwork":artist.image.as_ref().and_then(|image|image.for_px(300)).cloned().unwrap_or_default(),
                    "albums":artist.albums_count.unwrap_or(0)
                })).collect::<Vec<_>>()
            }))
        })
    })();
    to_java(&mut env, result_json(reply))
}

fn track_json(track: Track) -> serde_json::Value {
    let artwork = track
        .album
        .as_ref()
        .and_then(|album| album.image.for_px(150))
        .cloned()
        .unwrap_or_default();
    let album = track
        .album
        .as_ref()
        .map(|album| album.title.clone())
        .unwrap_or_default();
    json!({
        "id":track.id,"title":track.title,
        "artist":track.performer.map(|artist|artist.name).unwrap_or_default(),
        "album":album,"artwork":artwork,"duration":track.duration,
        "rate":track.maximum_sampling_rate,"bits":track.maximum_bit_depth
    })
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_loadAlbum(
    mut env: JNIEnv,
    _class: JClass,
    album_id: JString,
) -> jstring {
    let reply = (|| {
        let album_id = to_rust(&mut env, album_id)?;
        with_state(|state| {
            let album = state
                .runtime
                .block_on(state.client.get_album(&album_id))
                .map_err(|e| e.to_string())?;
            let album_title = album.title.clone();
            let album_artist = album.artist.name.clone();
            let album_artwork = album.image.for_px(300).cloned().unwrap_or_default();
            let tracks = album
                .tracks
                .map(|container| container.items)
                .unwrap_or_default();
            Ok(
                json!({"title":album_title,"tracks":tracks.into_iter().map(|track| {
                let mut value = track_json(track);
                if let Some(object) = value.as_object_mut() {
                    if object.get("album").and_then(|v|v.as_str()).unwrap_or_default().is_empty() {
                        object.insert("album".into(), json!(album_title));
                    }
                    if object.get("artist").and_then(|v|v.as_str()).unwrap_or_default().is_empty() {
                        object.insert("artist".into(), json!(album_artist));
                    }
                    if object.get("artwork").and_then(|v|v.as_str()).unwrap_or_default().is_empty() {
                        object.insert("artwork".into(), json!(album_artwork));
                    }
                }
                value
            }).collect::<Vec<_>>() }),
            )
        })
    })();
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_loadArtist(
    mut env: JNIEnv,
    _class: JClass,
    artist_id: jlong,
) -> jstring {
    let reply = with_state(|state| {
        let tracks = state
            .runtime
            .block_on(state.client.get_artist_tracks(artist_id as u64, 50, 0))
            .map_err(|e| e.to_string())?;
        Ok(json!({"tracks":tracks.items.into_iter().map(track_json).collect::<Vec<_>>() }))
    });
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_getPlaylists(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let reply = with_state(|state| {
        let playlists = state
            .runtime
            .block_on(state.client.get_user_playlists())
            .map_err(|e| e.to_string())?;
        Ok(json!({"playlists":playlists.into_iter().map(|playlist| {
            let artwork = playlist.images300.as_ref().and_then(|images| images.first()).cloned()
                .or_else(|| playlist.images.as_ref().and_then(|images| images.first()).cloned())
                .unwrap_or_default();
            json!({
                "id":playlist.id,"name":playlist.name,"description":playlist.description,
                "artwork":artwork,"tracks":playlist.tracks_count,
                "duration":playlist.duration,"owner":playlist.owner.name,
                "public":playlist.is_public
            })
        }).collect::<Vec<_>>() }))
    });
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_loadPlaylist(
    mut env: JNIEnv,
    _class: JClass,
    playlist_id: jlong,
) -> jstring {
    let reply = with_state(|state| {
        let playlist = state
            .runtime
            .block_on(state.client.get_playlist(playlist_id as u64))
            .map_err(|e| e.to_string())?;
        let title = playlist.name.clone();
        let tracks = playlist
            .tracks
            .map(|container| container.items)
            .unwrap_or_default();
        Ok(json!({"title":title,"tracks":tracks.into_iter().map(track_json).collect::<Vec<_>>() }))
    });
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_createPlaylist(
    mut env: JNIEnv,
    _class: JClass,
    name: JString,
) -> jstring {
    let reply = (|| {
        let name = to_rust(&mut env, name)?;
        let name = name.trim();
        if name.is_empty() {
            return Err("プレイリスト名を入力してください".to_string());
        }
        with_state(|state| {
            let playlist = state
                .runtime
                .block_on(state.client.create_playlist(name, None, false))
                .map_err(|e| e.to_string())?;
            Ok(json!({"id":playlist.id,"name":playlist.name}))
        })
    })();
    to_java(&mut env, result_json(reply))
}

fn inspect_flac(bytes: &[u8]) -> Result<(u32, u16, u8), String> {
    let source = Box::new(CursorSource(Cursor::new(bytes.to_vec()))) as Box<dyn MediaSource>;
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let probed = get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let track = probed
        .format
        .default_track()
        .ok_or("FLAC has no audio track")?;
    Ok((
        track
            .codec_params
            .sample_rate
            .ok_or("FLAC sample rate missing")?,
        track
            .codec_params
            .channels
            .map(|c| c.count() as u16)
            .unwrap_or(2),
        track.codec_params.bits_per_sample.unwrap_or(16) as u8,
    ))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_prepareTrack(
    mut env: JNIEnv,
    _class: JClass,
    track_id: jlong,
) -> jstring {
    let reply = (|| {
        let track_id = track_id as u64;
        if let Some(prepared) = with_state(|state| Ok(state.prepared.get(&track_id).cloned()))? {
            return Ok(
                json!({"trackId":prepared.track_id,"sampleRate":prepared.sample_rate,
                "channels":prepared.channels,"bitDepth":prepared.bit_depth,
                "bytes":prepared.bytes.len(),"cached":true}),
            );
        }
        let (client, handle) =
            with_state(|state| Ok((state.client.clone(), state.runtime.handle().clone())))?;
        let (bytes, _quality) = handle
            .block_on(qbz_qobuz::cmaf::download_full_with_quality(
                &client,
                track_id,
                Quality::UltraHiRes,
            ))
            .map_err(|e| e.to_string())?;
        let (sample_rate, channels, bit_depth) = inspect_flac(&bytes)?;
        if channels != 2 || !matches!(bit_depth, 16 | 24) {
            return Err(format!(
                "unsupported source PCM: {sample_rate} Hz {bit_depth}-bit {channels}ch"
            ));
        }
        let prepared = Prepared {
            bytes,
            sample_rate,
            channels,
            bit_depth,
            track_id,
        };
        let response = json!({"trackId":prepared.track_id,"sampleRate":sample_rate,"channels":channels,"bitDepth":bit_depth,"bytes":prepared.bytes.len()});
        with_state(|state| {
            if state.prepared.len() >= 3 && !state.prepared.contains_key(&track_id) {
                if let Some(oldest) = state.prepared.keys().next().copied() {
                    state.prepared.remove(&oldest);
                }
            }
            state.prepared.insert(track_id, prepared);
            Ok(())
        })?;
        Ok(response)
    })();
    to_java(&mut env, result_json(reply))
}

fn decode_to_usb(prepared: &Prepared, stream: &AndroidUsbDirectStream) -> Result<u64, String> {
    let source =
        Box::new(CursorSource(Cursor::new(prepared.bytes.clone()))) as Box<dyn MediaSource>;
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut probed = get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let track = probed
        .format
        .default_track()
        .ok_or("FLAC has no audio track")?;
    let track_id = track.id;
    let mut decoder = get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut frames = 0u64;
    loop {
        if STOP_REQUESTED.load(Ordering::Relaxed) {
            break;
        }
        while PAUSE_REQUESTED.load(Ordering::Relaxed) {
            if STOP_REQUESTED.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if STOP_REQUESTED.load(Ordering::Relaxed) {
            break;
        }
        let packet = match probed.format.next_packet() {
            Ok(v) => v,
            Err(SymphoniaError::IoError(_)) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                let spec = *audio.spec();
                let mut samples = SampleBuffer::<i32>::new(audio.frames() as u64, spec);
                samples.copy_interleaved_ref(audio);
                stream.write_i32(samples.samples())?;
                frames += samples.samples().len() as u64 / prepared.channels as u64;
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    stream.drain()?;
    Ok(frames)
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_playPrepared(
    mut env: JNIEnv,
    _class: JClass,
    track_id: jlong,
    fd: jint,
    endpoint: jint,
    feedback: jint,
    max_packet: jint,
    service_rate: jint,
) -> jstring {
    STOP_REQUESTED.store(false, Ordering::Relaxed);
    PAUSE_REQUESTED.store(false, Ordering::Relaxed);
    let prepared = with_state(|state| {
        state
            .prepared
            .get(&(track_id as u64))
            .cloned()
            .ok_or("track is not prepared".to_string())
    });
    let reply = prepared.and_then(|prepared| {
        let format=PcmFormat { sample_rate:prepared.sample_rate, channels:prepared.channels, subslot_bytes:prepared.bit_depth/8, bit_resolution:prepared.bit_depth };
        let stream=AndroidUsbDirectStream::new(AndroidUsbDirectConfig { java_fd:fd, data_endpoint:endpoint as u8, feedback_endpoint:feedback as u8, max_packet_size:max_packet as usize, service_rate:service_rate as u32, format })?;
        let frames=decode_to_usb(&prepared,&stream)?;
        Ok(json!({"frames":frames,"sampleRate":prepared.sample_rate,"bitDepth":prepared.bit_depth,"audioTrackUsed":false,"feedbackEndpoint":format!("0x{:02X}",feedback)}))
    });
    to_java(&mut env, result_json(reply))
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_stop(_env: JNIEnv, _class: JClass) {
    STOP_REQUESTED.store(true, Ordering::Relaxed);
    PAUSE_REQUESTED.store(false, Ordering::Relaxed);
}

#[no_mangle]
pub extern "system" fn Java_dev_qbz_android_NativeQbz_setPaused(
    _env: JNIEnv,
    _class: JClass,
    paused: jboolean,
) {
    PAUSE_REQUESTED.store(paused != 0, Ordering::Relaxed);
}
