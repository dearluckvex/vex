1. 因为宿主机开启了clash，我关闭clash后进行了尝试，[root@swimmingpool xtune]# curl -x http://127.0.0.1:1087 https://www.google.com -I
   HTTP/1.1 200 Connection Established


HTTP/2 200
content-type: text/html; charset=ISO-8859-1
content-security-policy-report-only: object-src 'none';base-uri 'self';script-src 'nonce-PM0L4DcVIA7XHkRgBDDm-g' 'strict-dynamic' 'report-sample' 'unsafe-eval' 'unsafe-inline' https: http:;report-uri https://csp.withgoogle.com/csp/gws/other-hp
accept-ch: Sec-CH-Prefers-Color-Scheme
p3p: CP="This is not a P3P policy! See g.co/p3phelp for more info."
date: Tue, 14 Apr 2026 15:37:25 GMT
server: gws
x-xss-protection: 0
x-frame-options: SAMEORIGIN
expires: Tue, 14 Apr 2026 15:37:25 GMT
cache-control: private
set-cookie: AEC=AaJma5szHk2jzC1fuXu_wqseKq_-DFb9fLKAKqZuTyVSIGjw-2wvmRg7m0k; expires=Sun, 11-Oct-2026 15:37:25 GMT; path=/; domain=.google.com; Secure; HttpOnly; SameSite=lax
set-cookie: NID=530=dZvqhAWgkjSUO1Lnlu7fAhmyth0T2_U8VQokcIznAgzM8VXPa38hMJERgErPhIURvnMaWiQz3mKPaJ-0qtPoydBB2S11tzINRVZAf4zIjlTtSWEIDHSJjE52_BKqm_rWj6-smWu4-MO4690RIHiuEbn1XGhoO-Cjv7hIQzvYOH4KLN5yoEoX6QEzx76zq_PYgKUDusRgJ0GspdPaN14p0aB2pV5xYA; expires=Wed, 14-Oct-2026 15:37:25 GMT; path=/; domain=.google.com; HttpOnly
set-cookie: __Secure-BUCKET=CIgB; expires=Sun, 11-Oct-2026 15:37:25 GMT; path=/; domain=.google.com; Secure; HttpOnly
alt-svc: h3=":443"; ma=2592000,h3-29=":443"; ma=2592000

2. 直接 curl google.com 一直没有反应