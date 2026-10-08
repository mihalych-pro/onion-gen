#!/usr/bin/env bash
#
# Builds a PDF from a Markdown document in docs/.
#
# The PDF is generated from the same Markdown the repository serves, and the
# diagrams are the same SVG files the Markdown references. Nothing is drawn
# twice, so the two cannot drift apart.
#
# The route is pandoc -> HTML -> Chrome. Chrome rather than a LaTeX engine
# because the diagrams are SVG with a `prefers-color-scheme` block in them: a
# browser renders that correctly and picks the light palette for print, while
# the usual PDF engines either rasterise the SVG or refuse it.
#
# Usage: build-pdf.sh docs/design/distributed-search.ru.md [more.md ...]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
[ -x "$CHROME" ] || CHROME="$(command -v google-chrome || command -v chromium || true)"

command -v pandoc >/dev/null || { echo "pandoc is not installed" >&2; exit 2; }
[ -n "$CHROME" ] && [ -x "$CHROME" ] || { echo "Chrome is not installed" >&2; exit 2; }
[ $# -ge 1 ] || { echo "usage: $0 <markdown> [...]" >&2; exit 2; }

css="$(mktemp -t onion-gen-pdf-css)"
trap 'rm -f "$css"' EXIT
cat > "$css" <<'CSS'
@page { size: A4; margin: 18mm 16mm 20mm; }
body {
  font-family: "IBM Plex Sans", "Helvetica Neue", Arial, sans-serif;
  font-size: 10.5pt; line-height: 1.55; color: #141e27; max-width: none;
}
h1 { font-size: 21pt; line-height: 1.15; margin: 0 0 4pt; letter-spacing: -0.01em; }
h2 { font-size: 13pt; margin: 18pt 0 6pt; padding-top: 6pt; border-top: 0.6pt solid #d4dce3;
     break-after: avoid; }
h1 + p { color: #4f5f6d; }
p, li { orphans: 3; widows: 3; }
code { font-family: "IBM Plex Mono", ui-monospace, Menlo, monospace; font-size: 0.88em;
       background: #eceff3; padding: 0.5pt 3pt; border-radius: 2pt; }
pre { background: #eceff3; padding: 8pt 10pt; border-radius: 3pt; font-size: 9pt;
      overflow-x: auto; break-inside: avoid; }
pre code { background: none; padding: 0; }
img { max-width: 100%; height: auto; display: block; margin: 10pt auto; break-inside: avoid; }
table { border-collapse: collapse; width: 100%; font-size: 9.5pt; margin: 8pt 0;
        break-inside: avoid; }
th, td { text-align: left; padding: 5pt 7pt; border-bottom: 0.6pt solid #d4dce3;
         vertical-align: top; }
th { font-size: 8.5pt; text-transform: uppercase; letter-spacing: 0.06em; color: #4f5f6d; }
a { color: #141e27; text-decoration: none; border-bottom: 0.6pt solid #bcc7d1; }
blockquote { border-left: 2pt solid #b0225c; margin: 8pt 0; padding: 2pt 0 2pt 10pt;
             color: #4f5f6d; }
CSS

for src in "$@"; do
	[ -f "$src" ] || { echo "no such file: $src" >&2; exit 2; }
	dir="$(cd "$(dirname "$src")" && pwd)"
	stem="$(basename "$src" .md)"
	html="$dir/.$stem.pdf.html"
	out="$ROOT/build/docs/$stem.pdf"
	mkdir -p "$(dirname "$out")"

	# The intermediate HTML lives beside the Markdown so that the relative
	# `diagrams/...` paths in it resolve for the browser exactly as they do
	# for the repository's own renderer.
	# --resource-path: pandoc resolves relative image paths against the working
	# directory, not against the source file, so without this the diagrams are
	# silently left out and the PDF comes out text-only.
	pandoc "$src" --standalone --embed-resources --css "$css" \
		--resource-path="$dir" --metadata pagetitle="$stem" -o "$html"

	"$CHROME" --headless --disable-gpu --no-sandbox \
		--no-pdf-header-footer --print-to-pdf-no-header \
		--print-to-pdf="$out" "file://$html" >/dev/null 2>&1

	rm -f "$html"
	printf '%s -> %s (%s KiB)\n' "$src" "${out#"$ROOT"/}" "$(( ($(wc -c < "$out") + 1023) / 1024 ))"
done
