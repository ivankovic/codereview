# Running the browser UI behind nginx

`codereview web` is a plain HTTP server meant for a loopback address. To reach it from a
browser on another machine, put nginx in front of it: nginx terminates TLS on your domain and
forwards to the loopback port, and codereview never speaks to the internet directly.

## Read this before you publish anything

The session token is the only thing between the internet and your repositories. Whoever has
it reads every file and every commit of every repository you serve, and writes to `REVIEW.md`
and `NOTES.md`.

**With the agent on, that person can also make it run commands in those repositories.** The
agent is Claude Code (or an ACP agent) started in the repository root, and it asks for
permission according to its own settings: a permission mode of `auto` approves most tool
calls without asking anyone. Serve with `--no-agent` unless you are the only person who can
reach the site and you accept what a leaked token means. `--no-agent` does not merely hide
the buttons: the routes are not served at all, and the page is told there is no agent.

There are no accounts and no per-repository access: one token opens everything. If you need
more than that, put nginx's own authentication in front of it as well, see the end of this
page.

## The service

Install the binary somewhere the service can reach it.

```sh
cargo install --path . --features web --locked   # or: make install
sudo cp ~/.cargo/bin/codereview /usr/local/bin/
```

Make a token and keep it out of the unit file, out of `ps`, and out of your shell history.

```sh
umask 077
printf 'CODEREVIEW_TOKEN=%s\n' "$(openssl rand -hex 32)" | sudo tee /etc/codereview.env
sudo chmod 600 /etc/codereview.env
```

Without `CODEREVIEW_TOKEN` every restart invents a new token and everybody's bookmark stops
working. The token must be at least 16 characters; the server refuses to start otherwise.

`/etc/systemd/system/codereview.service`:

```ini
[Unit]
Description=codereview browser UI
After=network-online.target
Wants=network-online.target

[Service]
Type=exec
User=review
Group=review
WorkingDirectory=/srv/review
EnvironmentFile=/etc/codereview.env
ExecStart=/usr/local/bin/codereview web \
    --host 127.0.0.1 --port 8765 --no-open --no-agent \
    --public-url https://review.example.com \
    /srv/review/repos
Restart=on-failure
RestartSec=2

# codereview reads the repositories and writes REVIEW.md and NOTES.md inside them.
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/srv/review
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
LockPersonality=yes
MemoryDenyWriteExecute=yes

[Install]
WantedBy=multi-user.target
```

Notes on that unit:

- The service user must own the repositories, or git refuses to touch them. If it does not,
  add them to `safe.directory` in that user's git config.
- `ProtectHome=yes` hides `/home`, so keep the repositories somewhere else, `/srv/review` here.
  Every path in `ExecStart` is a repository, a directory inside one, or a directory of
  checkouts, whose repositories are all opened. See the main README.
- Drop `MemoryDenyWriteExecute` and loosen `ProtectHome` if you ever serve **with** the agent:
  Claude Code runs on Node, which needs both.
- `systemctl restart` works cleanly; the server shuts down on SIGTERM.

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now codereview
systemctl status codereview          # prints the URL to open, token included
```

## The certificate

Point an `A` record (and `AAAA` if you have IPv6) at the machine, then start with nothing but
the plain HTTP server block, so that certbot has something to edit:

```nginx
server {
    listen 80;
    listen [::]:80;
    server_name review.example.com;
    root /var/www/html;
}
```

```sh
sudo certbot --nginx -d review.example.com
```

Certbot obtains the certificate, adds a TLS server block and a redirect, and installs a timer
that renews it. Replace what it wrote with the block below, keeping its `ssl_certificate`
paths.

## nginx

In the `http` block, a log format that drops the query string. The token appears in a query
exactly once, on a first visit, and there is no reason to keep it on disk:

```nginx
log_format noquery '$remote_addr - $remote_user [$time_local] '
                   '"$request_method $uri $server_protocol" $status $body_bytes_sent '
                   '"$http_referer" "$http_user_agent"';
```

The site itself:

```nginx
server {
    listen 80;
    listen [::]:80;
    server_name review.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name review.example.com;

    ssl_certificate     /etc/letsencrypt/live/review.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/review.example.com/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

    add_header Strict-Transport-Security "max-age=31536000" always;

    access_log /var/log/nginx/review.access.log noquery;

    # Diffs and file contents are JSON and compress well.
    gzip on;
    gzip_types application/json;
    gzip_min_length 1024;

    # Nothing is uploaded; a comment is a few hundred bytes.
    client_max_body_size 256k;

    location / {
        proxy_pass http://127.0.0.1:8765;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # A first diff of a large file can take a while; the page polls the agent every
        # half second but each request is short.
        proxy_read_timeout 120s;
    }
}
```

```sh
sudo nginx -t && sudo systemctl reload nginx
```

codereview needs nothing from the proxy: no WebSocket upgrade, no buffering changes, no
forwarded headers. It never builds an absolute URL from the request, which is why
`--public-url` is given on the command line instead: it decides the URL printed at start-up
and marks the session cookie `Secure` when it is an HTTPS one.

## Signing in

Open the URL the service log prints, once:

```
https://review.example.com/?t=<the token>
```

The server checks the token, sets a cookie that lasts 30 days, and redirects to the bare
domain, so the token is not left in the address bar, in the browser's history, or in the log
of every later request. Bookmark `https://review.example.com/` afterwards. When the cookie
expires, or in a new browser, open the token URL again.

The cookie only opens the page. Every API request carries the token in an `Authorization`
header instead, which a page on another site cannot set, so no other site can act as your
signed-in browser.

## If you want another lock in front

Both of these go inside the `location /` block.

Basic authentication, as a second factor over the token:

```sh
sudo htpasswd -c /etc/nginx/review.htpasswd marko
```

```nginx
auth_basic "codereview";
auth_basic_user_file /etc/nginx/review.htpasswd;
```

Or let only your own network in:

```nginx
allow 203.0.113.0/24;
allow 2001:db8::/32;
deny all;
```

## Things worth knowing

- Anyone signed in sees the filesystem path of every repository, in the page header and in
  the repository list. Serve from a directory whose name you do not mind showing.
- Each repository keeps its own agent process, started the first time somebody asks for one,
  and its own symbol index, built the first time somebody looks a name up. A directory of
  many large checkouts is cheap to start and grows as it is used.
- Logs go to the journal: `journalctl -u codereview -f`.
- To rotate the token, edit `/etc/codereview.env` and `systemctl restart codereview`. Every
  browser has to open the new token URL once.
