// crates/qbzd/src/cli/resolve.rs — the `qbzd resolve <URL>` verb (02 §2.3).
// Pure, client-side, no daemon and no network: turn a Qobuz share URL into the
// `kind:ID` token the other verbs accept (`qbzd resolve <url>` →
// `album:c9vd8vvvrbpkc`, pipeable into `qbzd play`/`album`/`artist`). Uses the
// pure qbz_qobuz::link_resolver (already a qbzd dependency). Exit 0 on a
// recognized link, 2 on an unrecognized one.
use qbz_qobuz::link_resolver::{resolve_link, ResolvedLink};

pub fn resolve(url: String) -> i32 {
    match resolve_link(&url) {
        Ok(link) => {
            println!("{}", token(&link));
            0
        }
        Err(_) => {
            eprintln!("error: unrecognized Qobuz URL");
            eprintln!("  → expected an open.qobuz.com album/track/artist/playlist link");
            2
        }
    }
}

fn token(link: &ResolvedLink) -> String {
    match link {
        ResolvedLink::OpenAlbum(id) => format!("album:{id}"),
        ResolvedLink::OpenTrack(id) => format!("track:{id}"),
        ResolvedLink::OpenArtist(id) => format!("artist:{id}"),
        ResolvedLink::OpenPlaylist(id) => format!("playlist:{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_formats_each_kind() {
        assert_eq!(token(&ResolvedLink::OpenAlbum("abc".into())), "album:abc");
        assert_eq!(token(&ResolvedLink::OpenTrack(42)), "track:42");
        assert_eq!(token(&ResolvedLink::OpenArtist(9)), "artist:9");
        assert_eq!(token(&ResolvedLink::OpenPlaylist(7)), "playlist:7");
    }
}
