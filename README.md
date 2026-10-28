# Sito LARD

Landing + bootstrap dell'installer.

- **Live**: <https://lard.francescocallegaro.com> — servita dal server via
  Cloudflare Tunnel (`lard-site`, unit `cloudflared-lard.service` +
  `lard-site-www.service`, static root = questa directory).
- **Mirror**: <https://francescokalle.github.io/lard/> (branch `gh-pages`,
  pubblicato con `make site-publish`; niente CNAME, il dominio è del tunnel).

Il one-liner è:

```bash
curl -fsSL https://lard.francescocallegaro.com/install.sh | bash
```

`install.sh` qui è solo il bootstrap: scarica il repo ed esegue quello vero.
