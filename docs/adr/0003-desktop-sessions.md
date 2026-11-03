# Desktop sessiyalari va token rotatsiyasi

Sana — 2026-yil 3-noyabr

Access token Ed25519 imzoli JWT shaklida. API imzo, algoritm, `kid`, `typ`, issuer, audience va muddatni tekshiradi. Himoyalangan so‘rovda PostgreSQL sessiya, qurilma va hisob holatini ham tasdiqlaydi. Bu logout, refresh qayta ishlatilishi va parol tiklanishi ortidan navbatdagi so‘rovdayoq tokenni rad etish imkonini beradi. Lokal sinov muddati access uchun 15 daqiqa, sessiya uchun dastlabki logindan 30 kun. Ochiq kalitlar bilan rotatsiya oldingi imzolarni o‘tish davrida tekshirishga imkon beradi. [JWT xavfsizlik amaliyoti](https://www.rfc-editor.org/rfc/rfc8725.html)

Refresh token 32 tasodifiy baytdan iborat. PostgreSQL faqat SHA-256 xeshini saqlaydi. Refresh sessiya qatorini bloklab, eski tokenni sarflaydi va yangisini shu tranzaksiyada qo‘shadi. Eski token xeshi sessiya muddati tugaguncha qoladi. Ishlatilgan token kelganda sessiya bekor qilinadi. Birinchi parallel so‘rov muvaffaqiyatli qaytsa ham, ikkinchisi qayta ishlatishni aniqlagach birinchi so‘rov olgan juftlik rad etiladi. Desktop bitta sessiya uchun bitta refresh so‘rovini faol tutadi. [Refresh rotatsiyasi bo‘yicha xavfsizlik amaliyoti](https://www.rfc-editor.org/rfc/rfc9700.html#section-4.14)

Login parol tekshiruvidan keyin hisob qatorini bloklaydi va tekshirilgan xesh joriyligini tasdiqlaydi. Parol tiklash shu hisobning faol sessiyalarini parol bilan bir tranzaksiyada bekor qiladi. Qurilma bekor qilinishi faqat egasining qurilmasiga va shu qurilmaga bog‘langan sessiyalarga ta’sir qiladi.
