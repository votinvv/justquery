// Build justquery-icons.ttf from icons_fixed/*.svg with fixed PUA codepoints.
// Uses fantasticon's own underlying libs directly because fantasticon's CLI
// hits the Windows path.join-vs-glob bug ("No SVGs found"). Run from _design_v2_1:
//   node build-font.js
const fs = require("fs");
const path = require("path");

const NPM_ROOT = path.join(process.env.APPDATA, "npm", "node_modules", "fantasticon", "node_modules");
const { SVGIcons2SVGFontStream } = require(path.join(NPM_ROOT, "svgicons2svgfont"));
const svg2ttf = require(path.join(NPM_ROOT, "svg2ttf"));

const codepoints = JSON.parse(fs.readFileSync("codepoints.json", "utf8"));
const SRC = "icons_fixed";
const OUT = path.join("build", "justquery-icons.ttf");

const fontStream = new SVGIcons2SVGFontStream({
  fontName: "justquery-icons",
  fontHeight: 1000,
  normalize: true,
  log: () => {},
});

let svgFont = "";
fontStream
  .on("data", (chunk) => (svgFont += chunk.toString()))
  .on("end", () => {
    const ttf = svg2ttf(svgFont, {});
    fs.mkdirSync("build", { recursive: true });
    fs.writeFileSync(OUT, Buffer.from(ttf.buffer));
    console.log(`wrote ${OUT} (${ttf.buffer.length} bytes, ${Object.keys(codepoints).length} glyphs)`);
  })
  .on("error", (e) => {
    console.error(e);
    process.exit(1);
  });

for (const [name, cp] of Object.entries(codepoints)) {
  const file = path.join(SRC, `${name}.svg`);
  if (!fs.existsSync(file)) {
    console.error(`missing ${file}`);
    process.exit(1);
  }
  const glyph = fs.createReadStream(file);
  glyph.metadata = { unicode: [String.fromCodePoint(cp)], name };
  fontStream.write(glyph);
}
fontStream.end();
