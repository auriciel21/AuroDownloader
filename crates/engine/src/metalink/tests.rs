use proptest::prelude::*;

use super::*;

const SHA256: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const SHA1: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
const MD5: &str = "098f6bcd4621d373cade4e832627b4f6";

fn v4(files: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <generator>essai</generator>
  {files}
</metalink>"#
    )
}

fn names(m: &Metalink) -> Vec<String> {
    m.files.iter().map(MetaFile::path).collect()
}

#[test]
fn metalink_4_sources_by_priority_with_the_strongest_checksum() {
    let doc = v4(&format!(
        r#"<file name="debian.iso">
    <size>792723456</size>
    <identity>Debian</identity>
    <hash type="md5">{MD5}</hash>
    <hash type="sha-256">{SHA256}</hash>
    <hash type="sha-1">{SHA1}</hash>
    <pieces length="262144" type="sha-1"><hash>{SHA1}</hash><hash>{SHA1}</hash></pieces>
    <url location="de" priority="20">https://b.example/debian.iso</url>
    <url priority="1">https://a.example/debian.iso?x=1&amp;y=2</url>
    <url>https://z.example/debian.iso</url>
    <url location="fr" priority="20">http://c.example/debian.iso</url>
    <url priority="5">ftp://f.example/debian.iso</url>
    <metaurl mediatype="torrent" priority="1">https://t.example/debian.iso.torrent</metaurl>
    <signature mediatype="application/pgp-signature">-----BEGIN PGP SIGNATURE-----</signature>
  </file>"#
    ));
    let m = parse(doc.as_bytes()).unwrap();
    assert_eq!(m.format, Format::V4);
    assert!(m.ignored.is_empty(), "{:?}", m.ignored);
    let f = &m.files[0];
    assert_eq!(f.name, "debian.iso");
    assert!(f.dir.is_empty());
    assert_eq!(f.size, Some(792_723_456));
    // La plus forte, pas celle des morceaux.
    assert_eq!(
        f.checksum,
        Some(Checksum::new(Algo::Sha256, SHA256).unwrap())
    );
    // Priorité croissante, ordre du document à égalité, sans priorité à la fin ; FTP et le
    // torrent laissés de côté ; `&amp;` lu.
    assert_eq!(
        f.urls,
        [
            "https://a.example/debian.iso?x=1&y=2",
            "https://b.example/debian.iso",
            "http://c.example/debian.iso",
            "https://z.example/debian.iso",
        ]
    );
    assert_eq!(f.dropped_urls, 1);
}

/// Empreintes de morceaux (F-12, SPEC §7.9) : les plus fortes qui se lisent et conviennent
/// à la taille ; rangs du Metalink 3 remis dans l'ordre ; rien d'illisible ne fait écarter
/// le fichier.
#[test]
fn piece_checksums_are_kept_when_they_fit_the_file() {
    let p = |c: char, n: usize| std::iter::repeat_n(c, n).collect::<String>();
    let doc = v4(&format!(
        r#"<file name="a.bin">
    <size>2500</size>
    <pieces length="1000" type="md5"><hash>{a}</hash><hash>{b}</hash><hash>{c}</hash></pieces>
    <pieces length="1000" type="sha-1"><hash>{d}</hash><hash>{e}</hash><hash>{f}</hash></pieces>
    <pieces length="1000" type="sha-256"><hash>{g}</hash><hash>{g}</hash></pieces>
    <url>https://a.example/a.bin</url>
  </file>
  <file name="b.bin">
    <size>2500</size>
    <hash type="sha-256">{SHA256}</hash>
    <pieces length="1000" type="sha-1"><hash>pas une empreinte</hash><hash>{d}</hash><hash>{d}</hash></pieces>
    <url>https://a.example/b.bin</url>
  </file>
  <file name="c.bin">
    <pieces length="7" type="sha-512"><hash>{h}</hash></pieces>
    <pieces length="7" type="md5"><hash>{a}</hash><hash>{b}</hash></pieces>
    <url>https://a.example/c.bin</url>
  </file>"#,
        a = p('a', 32),
        b = p('b', 32),
        c = p('c', 32),
        d = p('d', 40),
        e = p('e', 40),
        f = p('f', 40),
        g = p('0', 64),
        h = p('1', 128),
    ));
    let m = parse(doc.as_bytes()).unwrap();
    assert_eq!(names(&m), ["a.bin", "b.bin", "c.bin"]);
    // SHA-256 n'a que deux morceaux pour 2500 octets : SHA-1, la plus forte qui convient.
    let a = m.files[0].pieces.as_ref().unwrap();
    assert_eq!((a.algo(), a.length(), a.count()), (Algo::Sha1, 1000, 3));
    assert_eq!(a.hashes()[1], p('e', 40));
    // Une empreinte illisible : morceaux ignorés, le fichier et son empreinte restent.
    assert_eq!(m.files[1].pieces, None);
    assert!(m.files[1].checksum.is_some());
    // Sans taille annoncée : gardées (la taille du serveur décidera) ; SHA-512 inconnue.
    let c = m.files[2].pieces.as_ref().unwrap();
    assert_eq!((c.algo(), c.count()), (Algo::Md5, 2));

    let v3 = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<metalink version="3.0" xmlns="http://www.metalinker.org/">
 <files>
  <file name="ordre.iso">
   <size>20</size>
   <verification>
    <pieces length="10" type="sha1"><hash piece="1">{e}</hash><hash piece="0">{d}</hash></pieces>
   </verification>
   <resources><url type="http">http://a.example/ordre.iso</url></resources>
  </file>
  <file name="trou.iso">
   <size>20</size>
   <verification>
    <pieces length="10" type="sha1"><hash piece="0">{d}</hash><hash piece="2">{e}</hash></pieces>
   </verification>
   <resources><url type="http">http://a.example/trou.iso</url></resources>
  </file>
 </files>
</metalink>"#,
        d = p('d', 40),
        e = p('e', 40),
    );
    let m = parse(v3.as_bytes()).unwrap();
    let ordered = m.files[0].pieces.as_ref().unwrap();
    assert_eq!(ordered.hashes(), [p('d', 40), p('e', 40)]);
    // Un rang manquant : ignorées.
    assert_eq!(m.files[1].pieces, None);
}

#[test]
fn metalink_3_by_preference_without_torrent_addresses() {
    let doc = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<metalink version="3.0" xmlns="http://www.metalinker.org/" generator="MirrorBrain">
 <files>
  <file name="openSUSE.iso">
   <size>4096</size>
   <verification>
    <hash type="md5">{MD5}</hash>
    <hash type="sha1">{SHA1}</hash>
    <pieces length="1024" type="sha1"><hash piece="0">{SHA1}</hash></pieces>
   </verification>
   <resources>
    <url type="bittorrent" preference="100">http://t.example/openSUSE.iso.torrent</url>
    <url type="http" location="de" preference="99">http://a.example/openSUSE.iso</url>
    <url type="https" location="fr" preference="100">https://b.example/openSUSE.iso</url>
    <url type="ftp" preference="100">ftp://c.example/openSUSE.iso</url>
    <url location="us">https://d.example/openSUSE.iso</url>
   </resources>
  </file>
 </files>
</metalink>"#
    );
    let m = parse(doc.as_bytes()).unwrap();
    assert_eq!(m.format, Format::V3);
    let f = &m.files[0];
    assert_eq!(f.size, Some(4096));
    assert_eq!(f.checksum, Some(Checksum::new(Algo::Sha1, SHA1).unwrap()));
    assert_eq!(
        f.urls,
        [
            "https://b.example/openSUSE.iso",
            "http://a.example/openSUSE.iso",
            "https://d.example/openSUSE.iso",
        ]
    );
    // Le `.torrent` et FTP sont écartés par leur `type`, avant d'être comptés.
    assert_eq!(f.dropped_urls, 0);
    // Un seul morceau de 1024 octets pour 4096 : ne convient pas, ignoré (F-12).
    assert_eq!(f.pieces, None);
    // Version 3 sans espace de noms : reconnue par l'attribut.
    let bare = doc.replace(r#" xmlns="http://www.metalinker.org/""#, "");
    assert_eq!(parse(bare.as_bytes()).unwrap().format, Format::V3);
}

#[test]
fn one_task_per_file_with_folders_cleaned_and_bad_paths_set_aside() {
    let file = |name: &str| format!(r#"<file name="{name}"><url>https://a.example/x</url></file>"#);
    let doc = v4(&[
        file("debian/13/netinst.iso"),
        file("debian/13/SHA256SUMS"),
        file("../evil"),
        file("/etc/passwd"),
        file("C:evil.exe"),
        file("a//b"),
        file("./x"),
        // Même chemin qu'un fichier déjà retenu.
        file("debian/13/netinst.iso"),
        // Un chemin qui en prolonge un autre (un fichier ne peut être aussi un dossier).
        file("debian/13/SHA256SUMS/x"),
        r#"<file><url>https://a.example/x</url></file>"#.to_owned(),
        r#"<file name="sans-adresse.iso"><url>ftp://a.example/x</url></file>"#.to_owned(),
        r#"<file name="taille.iso"><size>douze</size><url>https://a.example/x</url></file>"#.to_owned(),
        format!(r#"<file name="faux.iso"><hash type="sha-256">{SHA1}</hash><url>https://a.example/x</url></file>"#),
        // SHA-512 seul : pas d'empreinte vérifiée, mais le fichier reste.
        r#"<file name="sha512.iso"><hash type="sha-512">00</hash><url>https://a.example/x</url></file>"#.to_owned(),
    ]
    .concat());
    let m = parse(doc.as_bytes()).unwrap();
    assert_eq!(
        names(&m),
        [
            "debian/13/netinst.iso",
            "debian/13/SHA256SUMS",
            "sha512.iso"
        ]
    );
    assert_eq!(m.files[0].dir, ["debian", "13"]);
    assert_eq!(m.files[0].name, "netinst.iso");
    assert_eq!(m.files[2].checksum, None);
    let reasons: Vec<(&str, &str)> = m
        .ignored
        .iter()
        .map(|i| (i.name.as_str(), i.reason.as_str()))
        .collect();
    assert_eq!(reasons.len(), 11, "{reasons:#?}");
    for (name, reason) in &reasons[..5] {
        assert!(reason.starts_with("nom refusé"), "{name} : {reason}");
    }
    assert!(reasons[5].1.starts_with("même chemin"), "{reasons:?}");
    assert!(reasons[6].1.starts_with("même chemin"), "{reasons:?}");
    assert_eq!(reasons[7], ("", "fichier sans nom"));
    assert_eq!(
        reasons[8],
        ("sans-adresse.iso", "aucune adresse HTTP ou HTTPS")
    );
    assert_eq!(reasons[9], ("taille.iso", "taille illisible"));
    assert!(
        reasons[10].1.contains("empreinte sha256 illisible"),
        "{reasons:?}"
    );
}

#[test]
fn at_most_sixteen_sources_the_best_ones() {
    let urls: String = (0..20)
        .map(|i| {
            format!(
                r#"<url priority="{}">https://m{i}.example/f</url>"#,
                100 - i
            )
        })
        .collect();
    let doc = v4(&format!(
        r#"<file name="f">{urls}<url>https://m0.example/f</url></file>"#
    ));
    let f = &parse(doc.as_bytes()).unwrap().files[0];
    assert_eq!(f.urls.len(), MAX_SOURCES);
    assert_eq!(f.urls[0], "https://m19.example/f");
    assert_eq!(f.urls[15], "https://m4.example/f");
    // 21 lues (dont un doublon), 16 gardées.
    assert_eq!(f.dropped_urls, 5);
}

#[test]
fn documents_refused_whole() {
    let refused = |doc: &str| parse(doc.as_bytes()).unwrap_err();
    assert_eq!(
        refused("<html><body>404</body></html>"),
        MetalinkError::NotMetalink
    );
    assert_eq!(
        refused(r#"<metalink xmlns="urn:autre"/>"#),
        MetalinkError::NotMetalink
    );
    assert_eq!(refused(""), MetalinkError::NotMetalink);
    assert!(matches!(refused("<metalink"), MetalinkError::Xml(_)));
    assert!(matches!(
        refused(r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="a"></metalink>"#),
        MetalinkError::Xml(_)
    ));
    // Entités déclarées (milliard de rires) ou externes : pas de DTD.
    let dtd = r#"<?xml version="1.0"?><!DOCTYPE m [<!ENTITY a "aaaa">]>
<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="&a;"><url>https://a/x</url></file></metalink>"#;
    assert!(matches!(refused(dtd), MetalinkError::Xml(_)));
    let unknown = v4(r#"<file name="x"><url>https://a/&b;</url></file>"#);
    assert!(matches!(refused(&unknown), MetalinkError::Xml(_)));
    assert_eq!(
        refused(&v4(r#"<file name="x"><url>ftp://a/x</url></file>"#)),
        MetalinkError::NoUsableFile(" (aucune adresse HTTP ou HTTPS)".into())
    );
    assert_eq!(refused(&v4("")), MetalinkError::NoUsableFile(String::new()));
    let big = format!("{}{}", v4(""), " ".repeat(MAX_LEN));
    assert_eq!(refused(&big), MetalinkError::TooLarge);
}

#[test]
fn files_beyond_the_limit_are_set_aside() {
    let files: String = (0..MAX_FILES + 3)
        .map(|i| format!(r#"<file name="f{i}"><url>https://a.example/{i}</url></file>"#))
        .collect();
    let m = parse(v4(&files).as_bytes()).unwrap();
    assert_eq!(m.files.len(), MAX_FILES);
    assert!(!m.ignored.is_empty());
    assert!(m.ignored.iter().all(|i| i.reason.starts_with("plus de")));
}

#[test]
fn character_references_and_cdata_are_read() {
    let doc = v4(
        r#"<file name="caf&#233;.iso"><url><![CDATA[https://a.example/x?a=1&b=2]]></url><url>https://b.example/caf&#xE9;.iso</url></file>"#,
    );
    let f = &parse(doc.as_bytes()).unwrap().files[0];
    assert_eq!(f.name, "café.iso");
    assert_eq!(
        f.urls,
        ["https://a.example/x?a=1&b=2", "https://b.example/café.iso"]
    );
}

proptest! {
    // Un document quelconque, ou un vrai document abîmé : jamais de panique, et ce qui est
    // retenu respecte les règles (noms sûrs, 16 adresses HTTP(S) au plus).
    #[test]
    fn any_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse(&bytes);
    }

    #[test]
    fn damaged_documents_keep_the_rules(cut in 0usize..600, at in 0usize..600, byte in any::<u8>()) {
        let doc = v4(&format!(
            r#"<file name="d/f.iso"><size>10</size><hash type="sha-256">{SHA256}</hash>
<url priority="2">https://a.example/f.iso</url><url priority="1">http://b.example/f.iso</url></file>
<file name="../g"><url>https://a.example/g</url></file>"#
        ));
        let mut bytes = doc.into_bytes();
        let at = at.min(bytes.len().saturating_sub(1));
        bytes[at] = byte;
        bytes.truncate(bytes.len().saturating_sub(cut));
        if let Ok(m) = parse(&bytes) {
            for f in &m.files {
                prop_assert!(f.urls.len() <= MAX_SOURCES && !f.urls.is_empty());
                prop_assert!(f.urls.iter().all(|u| crate::net::check_url(u).is_ok()));
                for part in f.dir.iter().chain(std::iter::once(&f.name)) {
                    prop_assert!(part != ".." && part != "." && !part.is_empty());
                    prop_assert_eq!(part, &sanitize(part, NameRules::native()));
                }
            }
        }
    }
}
