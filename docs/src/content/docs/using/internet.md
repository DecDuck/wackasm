---
title: Internet
description: Downloading files and programs with curl, and TCP with nc.
sidebar:
  order: 4
---

Install an OpenComputers **internet card** to use `curl` and `nc`. The
networking happens on the server, so OpenComputers' config decides what is
reachable. By default HTTP and TCP are enabled, and local and private
addresses (`localhost`, `192.168.x.x`, ...) are blocked.

## curl

```
curl [OPTIONS] URL
```

| option | does |
|---|---|
| `-o FILE` | saves the body to FILE instead of printing it |
| `-O` | saves it under the URL's file name: `.../tool.wasm` saves `tool.wasm` |
| `-i` | prints the status line and headers before the body |
| `-I` | sends a `HEAD` request and prints only the status line and headers |
| `-X METHOD` | the request method: `GET` by default, `POST` with `-d` |
| `-H "Name: value"` | adds a header (repeatable) |
| `-d DATA` | sends DATA as the body; `-d @FILE` sends a file |
| `-f` | exits with code 22 on an HTTP error status (400 and above) |

A URL without a scheme gets `http://`. Downloads go to the
[files disk](/wackasm/using/files/), relative to the current directory.

### Installing programs

Host a `.wasm` file anywhere the server can reach (a GitHub release works),
then download it into `/bin`:

```
/> cd /bin
/bin> curl -O https://example.com/mytool.wasm
saved 61089 bytes to mytool.wasm in 3.4s
/bin> cd /
/> mytool some args
```

Downloads come in 2 KiB chunks, one per server tick, so expect about
40 KB/s.

## nc

```
nc [-C] [-w SECS] HOST PORT
```

Connects, then sends each line you type as you press Enter and prints
whatever has arrived. After **Ctrl+D** it prints what keeps arriving, until
the other side closes the connection or nothing comes for `-w` seconds
(10 by default). `-C` ends lines with `\r\n`, as HTTP and most text
protocols expect:

```
/> nc -C example.com 80
connected to example.com:80; Ctrl+D ends input
GET / HTTP/1.0
Host: example.com

^D
HTTP/1.0 200 OK
...
```

`nc` sends a line only after you press Enter, so it suits line-based
protocols rather than live sessions.
