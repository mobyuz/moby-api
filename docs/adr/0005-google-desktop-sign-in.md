# Google orqali desktopga kirish

Sana — 2026-yil 3-noyabr

Google Web OAuth klienti API’ning qat’iy sozlangan callback manziliga qaytadi. API brauzerni desktop ochgan `127.0.0.1` portiga yo‘naltiradi. Tizim brauzeri va qisqa muddatli loopback tinglovchisi desktopga o‘z tranzaksiya holatini tekshirish va listener’ni darhol yopish imkonini beradi. [Google Web OAuth](https://developers.google.com/identity/protocols/oauth2/web-server) · [Native ilovalar uchun loopback callback](https://www.rfc-editor.org/rfc/rfc8252.html#section-7.3)

Google va desktop uchun ikkita mustaqil S256 PKCE tekshiruvi bor. API Google `state` xeshini bir martalik yozuvga bog‘laydi. Google verifier va nonce API’dagi klient siri hamda `state`dan alohida HMAC yorliqlari bilan hosil qilinadi. Google imzosi, issuer, audience, nonce va tasdiqlangan email tekshirilgach, API desktop challenge’iga bog‘langan xeshlangan kod yaratadi. Moby access va refresh tokenlari desktopning HTTPS API so‘roviga qaytadi. [Google ID token tekshiruvi](https://developers.google.com/identity/openid-connect/openid-connect#validatinganidtoken) · [PKCE](https://www.rfc-editor.org/rfc/rfc7636.html)

Google identifikatori `provider` va `sub` juftligi orqali topiladi. Email band bo‘lsa, mavjud Moby sessiyasi va joriy parol bilan alohida bog‘lash talab qilinadi. Google tasdig‘i bilan hisobni o‘chirishda yangi bir martalik kodning `sub` qiymati aynan joriy hisobga bog‘langan bo‘lishi kerak.
