# Running the browser UI behind nginx

`codereview web` is a plain HTTP server meant for a loopback address. To reach it from a
browser on another machine, put nginx in front of it: nginx terminates TLS on your domain and
forwards to the loopback port, and codereview itself never listens on a public address. With
the agent on, the agent still makes outbound connections of its own to whatever model it
uses.

## Read this before you publish anything

The password is the only thing between the internet and your repositories. Whoever knows it
reads every file and every commit of every repository you serve, and writes to `REVIEW.md`
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

Choose a password and store only its hash. `hash-password` asks twice without echoing, and
prints nothing but the hash:

```sh
codereview hash-password
```

```sh
umask 077
sudo tee /etc/codereview.env >/dev/null <<'EOF'
CODEREVIEW_PASSWORD_HASH='$argon2id$v=19$m=19456,t=2,p=1$...'
EOF
sudo chmod 600 /etc/codereview.env
```

Quote the value: it contains `$`. The hash is Argon2id, so the password cannot be recovered
from the file, and a stolen hash cannot be replayed as a password.

Without `CODEREVIEW_PASSWORD_HASH` the server falls back to a token in the URL, which is the
local default and is refused outright on a non-loopback address.

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
Environment=CODEREVIEW_CONFIG=/srv/review/config.toml
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
- `CODEREVIEW_CONFIG` matters here: the config file otherwise lives under the service user's
  home, which `ProtectHome=yes` hides, and picking a colour scheme on the page saves it. Put
  it somewhere in `ReadWritePaths`, as above.
- `ProtectHome=yes` hides `/home`, so keep the repositories somewhere else, `/srv/review` here.
  Every path in `ExecStart` is a repository, a directory inside one, or a directory of
  checkouts, whose repositories are all opened. See the main README.
- Drop `MemoryDenyWriteExecute` and loosen `ProtectHome` if you ever serve **with** the agent:
  Claude Code runs on Node, which needs both.
- `systemctl restart` works cleanly; the server shuts down on SIGTERM.
- What the hardening block protects is the **host**, not the repositories. Nothing there stops
  an agent from doing as it likes inside `ReadWritePaths`, and `RestrictAddressFamilies`
  leaves it full outbound network. Keep `/srv/review` to what you are willing to lose.

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now codereview
systemctl status codereview          # prints the URL to open
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

In the `http` block, a log format that drops the query string, and a rate limit for signing
in. codereview slows repeated wrong passwords itself, but a limit here costs nothing and also
covers everything else:

```nginx
log_format noquery '$remote_addr - $remote_user [$time_local] '
                   '"$request_method $uri $server_protocol" $status $body_bytes_sent '
                   '"$http_referer" "$http_user_agent"';

limit_req_zone $binary_remote_addr zone=signin:1m rate=10r/m;
server_tokens off;
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
    http2 on;                        # nginx 1.25.1 and later; before that: listen 443 ssl http2;
    server_name review.example.com;

    ssl_certificate     /etc/letsencrypt/live/review.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/review.example.com/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;
    ssl_protocols TLSv1.2 TLSv1.3;   # what certbot's include should already say; say it anyway
    ssl_stapling on;
    ssl_stapling_verify on;

    add_header Strict-Transport-Security "max-age=31536000" always;

    access_log /var/log/nginx/review.access.log noquery;

    # Diffs and file contents are JSON and compress well.
    gzip on;
    gzip_types application/json;
    gzip_min_length 1024;

    # Nothing is uploaded; a comment is a few hundred bytes.
    client_max_body_size 256k;

    location = /login {
        limit_req zone=signin burst=5 nodelay;
        proxy_pass http://127.0.0.1:8765;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

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

Open `https://review.example.com/`. The page is a sign-in form; the password is the one you
hashed. A session lasts a week in that browser, and **Sign out** in the header ends it at
once. Wrong passwords are counted, and after a few the server stops answering sign-in for a
while, longer each time, so the password cannot be found by asking repeatedly.

The cookie only opens the page. Every API request carries a separate per-session secret in an
`Authorization` header, which a page on another site cannot set, so no other site can act as
your signed-in browser, and the cookie on its own is worth nothing to one.

On a loopback run without a password, the printed URL carries a token instead; it opens a
session once and redirects to the bare path, and the token is never what the page holds.

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
- Logs go to the journal: `journalctl -u codereview -f`. With a password configured nothing
  secret is printed there. Failed API calls are logged in full and answered in brief, so the
  browser is not told which paths exist on the machine.
- Each repository's symbol index is built the first time somebody looks a name up, and a
  request that asks for every occurrence of a common name is answered with the first 500.
  Both are per repository, so a directory of many checkouts costs memory as it is used.
- To change the password, run `codereview hash-password` again, replace the value in
  `/etc/codereview.env` and `systemctl restart codereview`. Restarting ends every session, so
  everybody signs in again.
- The agent is started with this server's own secrets removed from its environment, so a
  command it runs cannot read the password hash. It does inherit everything else the service
  has.
