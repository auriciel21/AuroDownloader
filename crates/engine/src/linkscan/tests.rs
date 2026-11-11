use proptest::prelude::*;

use super::*;

fn base() -> Url {
    Url::parse("https://site.example/dossier/page.html").unwrap()
}

fn urls(links: &[Link]) -> Vec<&str> {
    links.iter().map(|l| l.url.as_str()).collect()
}

#[test]
fn links_of_a_page_resolved_against_its_address() {
    let html = r##"<!DOCTYPE html><html><head>
<link rel="stylesheet" href="style.css">
<script src="app.js"></script>
</head><body>
<!-- <a href="cache.iso">commentaire</a> -->
<a href="debian.iso">ISO</a>
<A HREF='/pub/notes.PDF#page=2'>Notes</A>
<a href=../archive.tar.gz?v=1&amp;x=2>archive</a>
<a href="https://autre.example/film.mkv">film</a>
<a href="index.php">page</a>
<a href="mailto:a@b.c">mail</a> <a href="javascript:void(0)">js</a> <a href="#haut">ancre</a>
<img src="//cdn.example/img/photo.jpg" srcset="photo-2x.jpg 2x, photo-3x.jpg 3x">
<video poster="affiche.png"><source src="clip.webm" type="video/webm"></video>
<a href="magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&amp;dn=Debian+13">torrent</a>
<a href="debian.iso">doublon</a>
<a href="/télécharger/bébé.zip">accents</a>
</body></html>"##;
    let links = from_html(html, &base());
    assert_eq!(
        urls(&links),
        [
            "https://site.example/dossier/debian.iso",
            "https://site.example/pub/notes.PDF",
            "https://site.example/archive.tar.gz?v=1&x=2",
            "https://autre.example/film.mkv",
            "https://site.example/dossier/index.php",
            "https://cdn.example/img/photo.jpg",
            "https://site.example/dossier/photo-2x.jpg",
            "https://site.example/dossier/photo-3x.jpg",
            "https://site.example/dossier/affiche.png",
            "https://site.example/dossier/clip.webm",
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Debian+13",
            "https://site.example/t%C3%A9l%C3%A9charger/b%C3%A9b%C3%A9.zip",
        ]
    );
    let notes = &links[1];
    assert_eq!(notes.name, "notes.PDF");
    assert_eq!(notes.ext.as_deref(), Some("pdf"));
    assert_eq!(notes.kind, LinkKind::File);
    assert_eq!(links[2].name, "archive.tar.gz");
    assert_eq!(links[2].ext.as_deref(), Some("gz"));
    assert_eq!(links[4].kind, LinkKind::Page);
    assert_eq!(links[10].kind, LinkKind::Magnet);
    assert_eq!(links[10].name, "Debian 13");
    assert_eq!(links[11].name, "bébé.zip");
}

#[test]
fn a_base_tag_and_addresses_written_in_scripts() {
    let html = r#"<head><base href="https://miroir.example/pub/"></head>
<a href="f.iso">f</a><base href="https://ignorée.example/">
<script>var files = ["https:\/\/cdn.example\/v\/clip.mp4", 'https://cdn.example/a.zip'];
// voir https://doc.example/guide.pdf.</script>
<p>Copie : (https://texte.example/b.7z), puis http://texte.example/c.rar;</p>"#;
    let links = from_html(html, &base());
    assert_eq!(
        urls(&links),
        [
            "https://miroir.example/pub/f.iso",
            "https://cdn.example/v/clip.mp4",
            "https://cdn.example/a.zip",
            "https://doc.example/guide.pdf",
            "https://texte.example/b.7z",
            "http://texte.example/c.rar",
        ]
    );
}

#[test]
fn addresses_of_a_text_or_an_imported_list() {
    let text = "Liste :\nhttps://a.example/1.iso\r\n\thttp://b.example/dir/  \n\
                \"https://c.example/x.zip\",\"Mon fichier\",12\n\
                magnet:?xt=urn:btih:abc\nmagnet:?dn=sans-empreinte\nftp://d.example/f.bin\n\
                www.e.example/f.iso https://a.example/1.iso HTTPS://F.EXAMPLE/G.TXT";
    let links = from_text(text);
    assert_eq!(
        urls(&links),
        [
            "https://a.example/1.iso",
            "http://b.example/dir/",
            "https://c.example/x.zip",
            "magnet:?xt=urn:btih:abc",
            "https://f.example/G.TXT",
        ]
    );
    // Sans nom de fichier : le nom de l'hôte, et une page.
    assert_eq!(links[1].name, "dir");
    assert_eq!(links[1].kind, LinkKind::Page);
    assert_eq!(links[3].name, "magnet");
    assert_eq!(links[4].ext.as_deref(), Some("txt"));
    // Colonnes d'un CSV, séparées par des points-virgules ou des virgules.
    let csv = from_text("url;nom\nhttps://a.example/x.iso;x\nhttps://b.example/y.zip,y,12\n");
    assert_eq!(
        urls(&csv),
        ["https://a.example/x.iso", "https://b.example/y.zip"]
    );
    let host = from_text("https://h.example/");
    assert_eq!(host[0].name, "h.example");
    assert_eq!(host[0].ext, None);
}

#[test]
fn names_from_the_server_are_cleaned() {
    let links =
        from_text("https://a.example/x/..%2F..%2Fetc%2Fpasswd https://a.example/%00a%3Ab.zip");
    for l in &links {
        assert_eq!(l.name, sanitize(&l.name, NameRules::native()), "{l:?}");
        assert!(!l.name.contains('/') && !l.name.contains('\0'), "{l:?}");
    }
}

#[test]
fn at_most_ten_thousand_links() {
    let text: String = (0..MAX_LINKS + 50)
        .map(|i| format!("https://a.example/{i}.bin\n"))
        .collect();
    assert_eq!(from_text(&text).len(), MAX_LINKS);
}

#[test]
fn entities_and_odd_markup() {
    assert_eq!(
        decode_entities("a&amp;b&#38;c&#x26;d&nbsp;&unknown;&"),
        "a&b&c&d &unknown;&"
    );
    // Balises tronquées, attributs sans valeur, guillemets non fermés : rien ne casse. Le
    // `<a` pris dans la balise `img` non fermée n'en est qu'un attribut, comme dans un
    // navigateur.
    let html = "<a href><a href=\"x.iso\" download><img src='y.png' <a href=\"z.zip";
    let links = from_html(html, &base());
    assert_eq!(
        urls(&links),
        [
            "https://site.example/dossier/x.iso",
            "https://site.example/dossier/y.png",
        ]
    );
    // Une adresse absolue dans un autre attribut (feuille de style, partage) n'est pas un
    // lien ; dans le texte de la page, oui.
    let html = r#"<link href="https://cdn.example/s.css"><meta content="https://site.example/x.png">
<p>Lien : https://site.example/y.zip</p><p>a < b https://site.example/z.7z</p>"#;
    assert_eq!(
        urls(&from_html(html, &base())),
        ["https://site.example/y.zip", "https://site.example/z.7z"]
    );
}

proptest! {
    // Un document quelconque : jamais de panique, et des liens toujours absolus, HTTP(S) ou
    // magnet, aux noms sûrs.
    #[test]
    fn any_text_never_panics(text in "\\PC{0,400}") {
        for links in [from_html(&text, &base()), from_text(&text)] {
            for l in links {
                prop_assert!(l.url.starts_with("http://") || l.url.starts_with("https://")
                    || l.url.starts_with("magnet:?"));
                prop_assert_eq!(&l.name, &sanitize(&l.name, NameRules::native()));
            }
        }
    }

    #[test]
    fn markup_like_text_never_panics(parts in proptest::collection::vec(
        prop_oneof![
            Just("<a href=".to_owned()), Just("\"".to_owned()), Just("'".to_owned()),
            Just(">".to_owned()), Just("<!--".to_owned()), Just("-->".to_owned()),
            Just("&amp;".to_owned()), Just("&#".to_owned()), Just("https://".to_owned()),
            Just("magnet:?xt=".to_owned()), Just("é".to_owned()), Just(" ".to_owned()),
            Just("f.iso".to_owned()), Just("<img srcset=".to_owned()), Just(",".to_owned()),
        ],
        0..60,
    )) {
        let text = parts.concat();
        let _ = from_html(&text, &base());
        let _ = from_text(&text);
    }
}
