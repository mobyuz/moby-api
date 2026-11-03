# Profil eksporti va roziliklarni boshqarish

Sana — 2026-yil 3-noyabr

Profil nomi ixtiyoriy. `PATCH /v1/me` nomning tashqi bo‘shliqlarini olib tashlaydi va 1–100 Unicode belgini qabul qiladi. Aniq `null` qiymati nomni tozalaydi.

Eksport bitta faqat o‘qish uchun ochilgan PostgreSQL snapshotidan hisob, qurilma, sessiya va rozilik metadatasini oladi. Har bir tur uchun maydonlar aniq tanlangan. API faylni so‘rov davomida yaratib qaytaradi. [PostgreSQL snapshot qoidalari](https://www.postgresql.org/docs/current/transaction-iso.html#XACT-REPEATABLE-READ)

Hisobni o‘chirish amaldagi sessiya va joriy parol tasdig‘ini talab qiladi. Parol tekshirilgandan keyin API hisob qatorini bloklab, parol xeshi va sessiya holatini qayta tekshiradi. Hisob, unga bog‘liq yozuvlar hamda email bo‘yicha limitlar bir tranzaksiyada o‘chadi. IP bo‘yicha umumiy limit odatdagi muddatida tugaydi. Hozirgi doira asosiy PostgreSQL bazasidagi ma’lumotlar.

Rozilikning har bir versiyasi uchun oxirgi holat saqlanadi. Qayta grant faol yozuvning vaqtini o‘zgartirmaydi. Qayta bekor qilish dastlabki bekor qilish vaqtini saqlaydi. Bekor qilingan rozilik yangidan berilsa, yangi qabul vaqti yoziladi. Bulutli ovoz roziligi alohida konfiguratsiya va yozuvga ega.
