use super::*;
use proptest::prelude::*;

fn page() -> Url {
    Url::parse("https://exemple.org/doc/index.html").unwrap()
}

fn found(html: &str) -> Vec<(String, Role)> {
    let bytes = html.as_bytes();
    links(bytes, &scan(bytes), &page())
        .into_iter()
        .map(|(u, r)| (u.to_string(), r))
        .collect()
}

fn s(u: &str, r: Role) -> (String, Role) {
    (u.to_owned(), r)
}

#[test]
fn every_kind_of_link_gets_its_role() {
    use Role::*;
    let html = r##"<!doctype html><html><head>
        <link rel="stylesheet" href="site.css">
        <link rel="alternate stylesheet" href="/alt.css">
        <LINK REL="Shortcut Icon" HREF="/favicon.ico">
        <link rel=preload as=font href=police.woff2 crossorigin>
        <link rel="preload" as="fetch" href="donnees.json">
        <link rel="modulepreload" href="m.js">
        <link rel="canonical" href="https://exemple.org/doc/">
        <script src="app.js"></script>
        <script>document.write('<a href="dans-un-script.html">');</script>
        <style>@import "imp.css"; body { background: url(fond.png) }</style>
        </head><body background="vieux.gif">
        <!-- <a href="commentaire.html"> -->
        <a href="a.html#haut">A</a> <a href="#ancre">ancre</a> <a href="mailto:x@y">m</a>
        <a href="https://autre.example/page">autre</a> <area href="carte.html">
        <iframe src="cadre.html"></iframe>
        <img src="i.png" srcset="i-2x.png 2x, i-3x.png 3x" data-src="lent.png" alt="">
        <picture><source srcset="p.webp 640w"><source src="pas-une-image.mp4"></picture>
        <video src="v.mp4" poster="affiche.jpg"></video><audio src="s.ogg"></audio>
        <input type="image" src="bouton.png"><input type="text" src="rien.png">
        <svg><image href="svg.png"/><use xlink:href="sprites.svg#icone"/></svg>
        <p style="background-image: url('style-attr.png')">t</p>
        <img src="data:image/png;base64,AAAA"> <a href="javascript:void(0)">j</a>
        <a href="a.html">encore</a>
        </body></html>"##;
    let base = "https://exemple.org/doc/";
    let u = |p: &str| format!("{base}{p}");
    assert_eq!(
        found(html),
        vec![
            s(&u("site.css"), Style),
            s("https://exemple.org/alt.css", Style),
            s("https://exemple.org/favicon.ico", Resource),
            s(&u("police.woff2"), Resource),
            s(&u("donnees.json"), Other),
            s(&u("m.js"), Resource),
            s("https://exemple.org/doc/", Other),
            s(&u("app.js"), Resource),
            s(&u("imp.css"), Style),
            s(&u("fond.png"), Resource),
            s(&u("vieux.gif"), Resource),
            s(&u("a.html"), Navigate),
            s("https://autre.example/page", Navigate),
            s(&u("carte.html"), Navigate),
            s(&u("cadre.html"), Navigate),
            s(&u("i.png"), Resource),
            s(&u("i-2x.png"), Resource),
            s(&u("i-3x.png"), Resource),
            s(&u("lent.png"), Resource),
            s(&u("p.webp"), Resource),
            s(&u("pas-une-image.mp4"), Other),
            s(&u("v.mp4"), Other),
            s(&u("affiche.jpg"), Resource),
            s(&u("s.ogg"), Other),
            s(&u("bouton.png"), Resource),
            s(&u("rien.png"), Other),
            s(&u("svg.png"), Resource),
            s(&u("sprites.svg"), Resource),
            s(&u("style-attr.png"), Resource),
        ]
    );
}

#[test]
fn the_base_tag_changes_where_relative_links_lead() {
    let html = r##"<head><base href="https://cdn.exemple.net/v2/"></head><img src="x.png"><a href="/p.html">"##;
    assert_eq!(
        found(html),
        vec![
            s("https://cdn.exemple.net/v2/x.png", Role::Resource),
            s("https://cdn.exemple.net/p.html", Role::Navigate),
        ]
    );
}

/// Les pages copiées : `index.html`, `a.html`, l'image `i.png`, le style `site.css`.
fn local_of(url: &Url) -> Option<String> {
    match url.as_str() {
        "https://exemple.org/doc/a.html" => Some("a.html".into()),
        "https://exemple.org/doc/i.png" => Some("../img/i%20x.png".into()),
        "https://exemple.org/doc/site.css" => Some("site.css".into()),
        "https://exemple.org/doc/index.html" => Some("index.html".into()),
        _ => None,
    }
}

fn rewrite_str(html: &str, charset: Option<&str>) -> String {
    String::from_utf8(rewrite(html.as_bytes(), &page(), charset, &mut local_of)).unwrap()
}

#[test]
fn copied_links_become_relative_and_the_others_absolute() {
    let html = concat!(
        r##"<html><head><meta charset="utf-8"><base href="/doc/">"##,
        r##"<link rel="stylesheet" href="site.css" integrity="sha384-x" crossorigin>"##,
        r##"<script src="app.js" integrity="sha384-y"></script></head>"##,
        r##"<body><a href="a.html#s2">A</a> <a href='b.html?x=1&amp;y=2'>B</a> "##,
        r##"<a href=a.html>nu</a> <a href="#top">t</a> <a href="mailto:a@b">m</a> "##,
        r##"<img src="i.png" srcset="i.png 1x, grand.png 2x">"##,
        r##"<p style="background: url(&quot;i.png&quot;)">x</p>"##,
        r##"<style>a { background: url(i.png) } b { background: url(/z.png) }</style>"##,
        r##"</body></html>"##
    );
    assert_eq!(
        rewrite_str(html, Some("iso-8859-1")),
        concat!(
            r##"<html><head><meta charset="utf-8"><base >"##,
            r##"<link rel="stylesheet" href="site.css"  crossorigin>"##,
            r##"<script src="https://exemple.org/doc/app.js" integrity="sha384-y"></script></head>"##,
            r##"<body><a href="a.html#s2">A</a> <a href='https://exemple.org/doc/b.html?x=1&amp;y=2'>B</a> "##,
            r##"<a href="a.html">nu</a> <a href="#top">t</a> <a href="mailto:a@b">m</a> "##,
            r##"<img src="../img/i%20x.png" srcset="../img/i%20x.png 1x, https://exemple.org/doc/grand.png 2x">"##,
            r##"<p style="background: url(&quot;../img/i%20x.png&quot;)">x</p>"##,
            r##"<style>a { background: url("../img/i%20x.png") } b { background: url("https://exemple.org/z.png") }</style>"##,
            r##"</body></html>"##
        )
    );
}

#[test]
fn the_charset_of_the_header_is_written_only_if_the_page_says_none() {
    let bare = "<html><head><title>t</title></head></html>";
    assert_eq!(
        rewrite_str(bare, Some("windows-1252")),
        "<html><head><meta charset=\"windows-1252\"><title>t</title></head></html>"
    );
    assert_eq!(rewrite_str(bare, None), bare);
    let said = "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head></html>";
    assert_eq!(rewrite_str(said, Some("utf-8")), said);
    // Sans `<head>`, rien n'est ajouté (un `<meta>` avant le doctype changerait le rendu).
    assert_eq!(rewrite_str("<p>x</p>", Some("utf-8")), "<p>x</p>");
    // Valeur nettoyée.
    assert_eq!(
        rewrite_str("<head></head>", Some("utf-8\"><script>")),
        "<head><meta charset=\"utf-8script\"></head>"
    );
}

#[test]
fn bytes_that_are_not_utf8_are_kept_and_utf16_is_not_rewritten() {
    let mut html = b"<p>caf\xe9</p><img src=\"i.png\" alt=\"\xe9t\xe9\">".to_vec();
    let out = rewrite(&html, &page(), None, &mut local_of);
    let expected = b"<p>caf\xe9</p><img src=\"../img/i%20x.png\" alt=\"\xe9t\xe9\">".to_vec();
    assert_eq!(out, expected);
    assert!(rewritable(&html));
    html = vec![0xFF, 0xFE, b'<', 0, b'p', 0];
    assert!(!rewritable(&html));
    assert!(!rewritable(&[b'<', 0, b'p', 0]));
}

#[test]
fn script_contents_are_never_rewritten() {
    let html = r##"<script>var a = '<img src="i.png">';</script><img src="i.png">"##;
    assert_eq!(
        rewrite_str(html, None),
        r##"<script>var a = '<img src="i.png">';</script><img src="../img/i%20x.png">"##
    );
}

proptest! {
    /// N'importe quels octets : ni la lecture ni la réécriture ne paniquent, et une page
    /// sans lien ressort telle quelle.
    #[test]
    fn any_bytes_are_read_and_rewritten_safely(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let sc = scan(&bytes);
        let _ = links(&bytes, &sc, &page());
        let out = rewrite(&bytes, &page(), None, &mut |_| None);
        if sc.attrs.is_empty() && sc.blocks.is_empty() {
            prop_assert_eq!(out, bytes);
        }
    }

    /// Une page faite de texte, de balises sans lien et de liens : la réécriture ne
    /// change que la valeur des liens, guillemets d'origine gardés.
    #[test]
    fn only_link_values_change(chunks in proptest::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9 .,;]{0,20}".prop_map(|t| (t.clone(), t)),
            "[a-z]{1,8}".prop_map(|n| (format!("<a href=\"{n}.html\">"), format!("<a href=\"L/{n}.html\">"))),
            "[a-z]{1,8}".prop_map(|n| (format!("<img alt=x src='{n}.png'>"), format!("<img alt=x src='L/{n}.png'>"))),
            "[a-z]{1,8}".prop_map(|n| (format!("<p class=\"{n}\">"), format!("<p class=\"{n}\">"))),
            "[a-z ]{0,10}".prop_map(|c| (format!("<!-- <a href=\"{c}\"> -->"), format!("<!-- <a href=\"{c}\"> -->"))),
        ],
        0..30,
    )) {
        let input: String = chunks.iter().map(|(i, _)| i.as_str()).collect();
        let expected: String = chunks.iter().map(|(_, o)| o.as_str()).collect();
        let out = rewrite(input.as_bytes(), &page(), None, &mut |u| {
            Some(format!("L/{}", u.path_segments().unwrap().next_back().unwrap()))
        });
        prop_assert_eq!(String::from_utf8(out).unwrap(), expected);
    }
}
