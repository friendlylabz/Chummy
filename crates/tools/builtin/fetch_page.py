# /// script
# requires-python = ">=3.14"
# dependencies = ["beautifulsoup4==4.13.4"]
#
# [tool.chummy]
# name = "fetch_page"
# title = "Fetch web page"
# version = "1.0.0"
# network = true
# timeout = 60
# ///
"""Fetches a web page and returns its readable text."""

from bs4 import BeautifulSoup

import chummy


@chummy.tool
def fetch_page(url: str, max_chars: int = 20000) -> str:
    """Get the readable text of a web page.

    url: the full address, starting with https://
    max_chars: stop after this many characters
    """
    if not url.startswith(("http://", "https://")):
        url = "https://" + url.strip()
    r = chummy.http.get(url).raise_for_status()
    kind = r.headers.get("content-type", "")
    if "html" not in kind:
        text = r.text
    else:
        soup = BeautifulSoup(r.text, "html.parser")
        for tag in soup(["script", "style", "noscript", "svg", "nav", "footer", "header", "form"]):
            tag.decompose()
        title = soup.title.get_text(strip=True) if soup.title else ""
        lines = [l.strip() for l in soup.get_text("\n").splitlines()]
        body = "\n".join(l for l in lines if l)
        text = f"{title}\n\n{body}" if title else body
    if len(text) > max_chars:
        text = text[:max_chars] + f"\n[… cut at {max_chars} characters]"
    return f"{r.url}\n\n{text}"
