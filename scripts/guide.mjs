// Guide de l'utilisateur (phase 7, étapes 9c et 9d) : docs/guide/<langue>/*.md, en Markdown
// pour GitHub, et sa copie dans l'application, ui/src/guide/<langue>.html : un fragment HTML
// sans script, où tout le texte est échappé, ouvert par la boîte « Guide » de la page.
//
//   node scripts/guide.mjs           refait ui/src/guide/*.html
//   node scripts/guide.mjs --check   vérifie liens, ancres, structure et fragments (CI)
//
// Markdown reconnu : titres #, ##, ###, paragraphes, listes à puces et numérotées (emboîtées
// par deux espaces), blocs de code ```, tableaux, `code`, **gras**, [liens](cible). Dans la
// copie de l'application, un lien vers un autre fichier du guide devient une ancre interne ;
// un lien hors du guide devient du texte suivi de son adresse (la page ne quitte jamais
// l'application).
import { readFileSync, readdirSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
export const LANGS = ["fr", "en"];
const REPO = "https://github.com/auriciel21/AuroDownloader/blob/main";

/** Ancre d'un titre, comme GitHub : minuscules, lettres, chiffres, espaces et tirets. */
export function slug(text) {
  return plain(text).toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, "").trim().replace(/\s/g, "-");
}

/** Texte d'un fragment Markdown, sans sa mise en forme. */
function plain(text) {
  return text.replace(/\[([^\]]*)\]\([^)]*\)/g, "$1").replace(/[`*]/g, "");
}

export function escape(text) {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Fichiers d'une langue, dans l'ordre : README.md d'abord, puis 01-…, 02-… */
export function files(lang) {
  const dir = join(ROOT, "docs/guide", lang);
  return readdirSync(dir).filter((f) => f.endsWith(".md")).sort((a, b) =>
    a === "README.md" ? -1 : b === "README.md" ? 1 : a.localeCompare(b));
}

function read(lang, file) {
  return readFileSync(join(ROOT, "docs/guide", lang, file), "utf8").replace(/\r\n/g, "\n");
}

/** Titres d'un fichier : niveau, texte, ancre. */
export function headings(markdown) {
  const out = [];
  let code = false;
  for (const line of markdown.split("\n")) {
    if (line.startsWith("```")) code = !code;
    const m = !code && /^(#{1,3}) (.+)$/.exec(line);
    if (m) out.push({ level: m[1].length, text: m[2], anchor: slug(m[2]) });
  }
  return out;
}

/** Liens d'un fichier (hors blocs de code). */
export function links(markdown) {
  const out = [];
  let code = false;
  for (const line of markdown.split("\n")) {
    if (line.startsWith("```")) code = !code;
    if (code) continue;
    for (const m of line.replace(/`[^`]*`/g, "").matchAll(/\[([^\]]*)\]\(([^)]+)\)/g)) out.push(m[2]);
  }
  return out;
}

/** Fautes du guide : liens internes cassés, ancres absentes, langues de structure différente. */
export function check() {
  const faults = [];
  for (const lang of LANGS) {
    const names = files(lang);
    for (const file of names) {
      for (const target of links(read(lang, file))) {
        if (/^[a-z]+:/.test(target)) continue;
        const [path, anchor] = target.split("#");
        // Un autre fichier du guide (même dossier) : il existe, et l'ancre y est.
        if (!path || (path.endsWith(".md") && !path.includes("/"))) {
          const other = path || file;
          if (!names.includes(other)) faults.push(`${lang}/${file} : ${target} introuvable`);
          else if (anchor && !headings(read(lang, other)).some((h) => h.anchor === anchor)) {
            faults.push(`${lang}/${file} : ancre ${target} absente`);
          }
          continue;
        }
        // Ailleurs dans le dépôt : le fichier ou le dossier existe.
        if (!existsSync(join(ROOT, "docs/guide", lang, path))) faults.push(`${lang}/${file} : ${target} introuvable`);
      }
    }
  }
  // Même structure dans les deux langues : mêmes fichiers (par leur numéro), même nombre de
  // titres de chaque niveau dans chacun.
  const [a, b] = LANGS.map((lang) => files(lang));
  if (a.length !== b.length) faults.push(`${a.length} fichiers en ${LANGS[0]}, ${b.length} en ${LANGS[1]}`);
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    if (a[i].slice(0, 3) !== b[i].slice(0, 3)) faults.push(`fichiers ${a[i]} et ${b[i]} : numéros différents`);
    const count = (lang, f) => headings(read(lang, f)).map((h) => h.level).join("");
    if (count(LANGS[0], a[i]) !== count(LANGS[1], b[i])) faults.push(`${a[i]} et ${b[i]} : titres différents`);
  }
  return faults;
}

// ---------- Conversion en HTML, pour l'application ----------

export function inline(text, ctx) {
  let out = "";
  const re = /`([^`]+)`|\*\*([^*]+)\*\*|!\[([^\]]*)\]\(([^)]+)\)|\[([^\]]+)\]\(([^)]+)\)/g;
  let last = 0;
  for (const m of text.matchAll(re)) {
    out += escape(text.slice(last, m.index));
    if (m[1] !== undefined) out += `<code>${escape(m[1])}</code>`;
    else if (m[2] !== undefined) out += `<strong>${inline(m[2], ctx)}</strong>`;
    // Image (capture d'écran) : son texte alternatif seul, l'application reste légère.
    else if (m[3] !== undefined) out += `<em class="guide-image">${escape(m[3])}</em>`;
    else out += link(m[5], m[6], ctx);
    last = m.index + m[0].length;
  }
  return out + escape(text.slice(last));
}

function link(label, target, ctx) {
  const [path, anchor] = target.split("#");
  const local = !/^[a-z]+:/.test(target) && (!path || (path.endsWith(".md") && !path.includes("/")));
  if (local) {
    const file = path || ctx.file;
    const id = anchor ? `${ctx.prefix(file)}-${anchor}` : ctx.prefix(file);
    return `<a href="#${escape(id)}">${inline(label, ctx)}</a>`;
  }
  // Hors du guide : du texte et son adresse, la page ne quitte pas l'application.
  const url = /^[a-z]+:/.test(target) ? target : `${REPO}/docs/guide/${ctx.lang}/${target}`.replace(/\/[^/]+\/\.\.\//g, "/").replace(/\/[^/]+\/\.\.\//g, "/");
  return `${inline(label, ctx)} (<span class="guide-url">${escape(url)}</span>)`;
}

function table(rows, ctx) {
  const cells = (row) => row.replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
  const [head, , ...body] = rows;
  return `<table><thead><tr>${cells(head).map((c) => `<th scope="col">${inline(c, ctx)}</th>`).join("")}</tr></thead>`
    + `<tbody>${body.map((r) => `<tr>${cells(r).map((c) => `<td>${inline(c, ctx)}</td>`).join("")}</tr>`).join("")}</tbody></table>`;
}

/** Listes emboîtées : lignes `- ` ou `1. `, indentées de deux espaces par niveau. */
function list(lines, ctx) {
  const items = [];
  for (const line of lines) {
    const m = /^( *)(?:- |\d+\. )(.*)$/.exec(line);
    if (m) items.push({ depth: m[1].length, ordered: !/^ *- /.test(line), text: m[2], children: [] });
    else if (items.length) items[items.length - 1].text += ` ${line.trim()}`;
  }
  const render = (start, depth) => {
    let html = "";
    let i = start;
    const ordered = items[start].ordered;
    while (i < items.length && items[i].depth >= depth) {
      if (items[i].depth > depth) {
        const [inner, next] = render(i, items[i].depth);
        html = html.replace(/<\/li>$/, `${inner}</li>`);
        i = next;
        continue;
      }
      html += `<li>${inline(items[i].text, ctx)}</li>`;
      i++;
    }
    return [`<${ordered ? "ol" : "ul"}>${html}</${ordered ? "ol" : "ul"}>`, i];
  };
  return render(0, items[0].depth)[0];
}

function body(markdown, ctx) {
  const lines = markdown.split("\n");
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    if (line.trimStart().startsWith("```")) {
      const indent = line.indexOf("```");
      const code = [];
      i++;
      while (i < lines.length && !lines[i].trimStart().startsWith("```")) code.push(lines[i++].slice(indent));
      i++;
      out.push(`<pre><code>${escape(code.join("\n"))}</code></pre>`);
      continue;
    }
    const h = /^(#{1,3}) (.+)$/.exec(line);
    if (h) {
      // Un niveau de plus : la boîte « Guide » a son propre titre de niveau 2.
      const level = h[1].length + 1;
      const id = h[1].length === 1 ? ctx.prefix(ctx.file) : `${ctx.prefix(ctx.file)}-${slug(h[2])}`;
      out.push(`<h${level} id="${escape(id)}" tabindex="-1">${inline(h[2], ctx)}</h${level}>`);
      i++;
      continue;
    }
    if (line.startsWith("|")) {
      const rows = [];
      while (i < lines.length && lines[i].startsWith("|")) rows.push(lines[i++]);
      out.push(table(rows, ctx));
      continue;
    }
    if (/^ *(?:- |\d+\. )/.test(line)) {
      const block = [];
      while (i < lines.length && lines[i].trim() && !lines[i].trimStart().startsWith("```")) block.push(lines[i++]);
      // Un bloc de code dans un élément de liste : la liste s'arrête, le code suit.
      out.push(list(block, ctx));
      continue;
    }
    const para = [];
    while (i < lines.length && lines[i].trim() && !/^(#{1,3} |\||```| *(?:- |\d+\. ))/.test(lines[i])) para.push(lines[i++].trim());
    out.push(`<p>${inline(para.join(" "), ctx)}</p>`);
  }
  return out.join("\n");
}

/** Fragment HTML d'une langue : une section par fichier. */
export function render(lang) {
  const names = files(lang);
  const prefix = (file) => `guide-${file.replace(/\.md$/, "").toLowerCase()}`;
  const sections = names.map((file) => {
    const ctx = { lang, file, prefix };
    return `<section aria-labelledby="${prefix(file)}">\n${body(read(lang, file), ctx)}\n</section>`;
  });
  return `<!-- Fait par scripts/guide.mjs depuis docs/guide/${lang} : ne pas modifier à la main. -->\n${sections.join("\n")}\n`;
}

export function output(lang) {
  return join(ROOT, "ui/src/guide", `${lang}.html`);
}

// Lancé directement (pas importé par ses tests) : même détection que scripts/latest-json.mjs.
if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  if (process.argv.includes("--check")) {
    const faults = check();
    for (const lang of LANGS) {
      const now = existsSync(output(lang)) ? readFileSync(output(lang), "utf8") : "";
      if (now !== render(lang)) faults.push(`ui/src/guide/${lang}.html pas à jour : node scripts/guide.mjs`);
    }
    if (faults.length) {
      console.error(faults.join("\n"));
      process.exit(1);
    }
    console.log("guide : liens, ancres, structure et copies de l'application à jour");
  } else {
    mkdirSync(join(ROOT, "ui/src/guide"), { recursive: true });
    for (const lang of LANGS) writeFileSync(output(lang), render(lang));
    console.log(`fait : ${LANGS.map((l) => `ui/src/guide/${l}.html`).join(", ")}`);
  }
}
