# Using as supervisord

**`microinit supervise`** is for **containers** and **servers** without full PID-1 init (no early-boot, no getty). It does what **supervisord** does: keep processes alive, start/stop from the CLI, read logs.

When **`microinit init`** runs as PID 1 (`/sbin/init`) on a host system. In Docker or on a VM:

```bash
microinit supervise --config /etc/microinit/microinit.json
```

Build locally or pull a published image (see project README). Example:

```bash
docker run --rm -v "$PWD/config:/etc/microinit" microinit:main
```

---

## Example: PHP-FPM + NGINX

Classic stack: **PHP-FPM** on port 9000, **NGINX** serves HTTP and talks to PHP-FPM. NGINX must start **after** PHP-FPM.

Config split into:

- a small **main** file (socket, logs)
- **drop-ins** in subfolders (easy to add more services)
- **hot reload** — edit JSON, save, no container restart

### File layout

```text
/etc/microinit/
  microinit.json
  microinit.d/
    services/
      web/
        php-fpm.json
        nginx.json
```

### Main file — `microinit.json`

```json
{
  "version": 1,
  "socket": "/run/microinit.sock",
  "logs": {
    "lines": 500,
    "logToFiles": true,
    "dir": "/var/log/microinit"
  },
  "services": []
}
```

Services live only in drop-ins — the main file stays short.

### PHP-FPM — `microinit.d/services/web/php-fpm.json`

```json
{
  "services": [
    {
      "name": "php-fpm",
      "enabled": true,
      "daemon": true,
      "restart": true,
      "restartBackoff": 2,
      "startWaitSecs": 1,
      "shutdownWaitSecs": 10,
      "dependsOn": [],
      "startCmd": "exec /usr/sbin/php-fpm8.2 --nodaemonize --fpm-config /etc/php8/php-fpm.conf",
      "stopCmd": "killall php-fpm8.2",
      "cwd": "/",
      "livenessProbe": {
        "tcpAddr": "127.0.0.1:9000",
        "interval": 30,
        "timeout": 3
      }
    }
  ]
}
```

- **`exec`** and **`--nodaemonize`** — PHP-FPM in the foreground; microinit tracks the PID.
- **`livenessProbe`** — checks port 9000.

### NGINX — `microinit.d/services/web/nginx.json`

```json
{
  "services": [
    {
      "name": "nginx",
      "enabled": true,
      "daemon": true,
      "restart": true,
      "restartBackoff": 2,
      "startWaitSecs": 1,
      "shutdownWaitSecs": 10,
      "dependsOn": ["php-fpm"],
      "startCmd": "exec /usr/sbin/nginx -g 'daemon off;'",
      "stopCmd": "nginx -s quit",
      "cwd": "/",
      "livenessProbe": {
        "httpUrl": "http://127.0.0.1:8080/health",
        "httpAcceptedCodes": [200],
        "interval": 30,
        "timeout": 5
      }
    }
  ]
}
```

- **`dependsOn: ["php-fpm"]`** — NGINX stays in `waiting_for_dependency` until PHP-FPM is `running`.
- **`daemon off`** — NGINX in the foreground (supervisord-style).

---

## Hot reload in practice

microinit watches the main file and everything under `microinit.d/services/**/*.json`.

### Add a service without restarting the container

e.g. `microinit.d/services/web/extra-app.json`:

```json
{
  "services": [
    {
      "name": "extra-app",
      "daemon": true,
      "restart": true,
      "startWaitSecs": 1,
      "dependsOn": ["php-fpm"],
      "startCmd": "exec /usr/sbin/my-extra-daemon"
    }
  ]
}
```

Save the file. Within about a second:

```bash
microinit list
```

`extra-app` should start (after `php-fpm` is ready).

### Change PHP-FPM settings

Edit `php-fpm.json` — microinit detects a definition change and **restarts** `php-fpm`. NGINX keeps running unless you change its JSON too or run `microinit restart nginx`.

### Invalid JSON

Broken JSON keeps the **previous** config. Check:

```bash
microinit logs --lines 20
```

---

## Everyday commands

```bash
microinit list
microinit restart nginx
microinit stop php-fpm
microinit logs nginx --follow
```

Socket defaults to `/run/microinit.sock`; use `--socket` on the daemon and clients if needed.

---

## supervise vs init

| | `supervise` | `init` (host PID 1) |
|---|-------------|---------------------|
| Early-boot, `/data` | No | Yes |
| Getty | No | Yes |
| Logs on tty2/tty3 | No (ring + files + IPC) | Yes |
| Supervision + hot reload | Yes | Yes |
| `shutdown -r` | Stop services, exit | Stop, umount, reboot |

---

## Further reading

- [Configuration](configuration.md) — drop-ins, JSON fields  
- [Service lifecycle](service-lifecycle.md) — waiting on dependencies  
- [Operator guide](operator.md)  
