# Hisobni email orqali tasdiqlash

Sana — 2026-yil 3-noyabr

Ro‘yxatdan o‘tishda har bir tasdiqlash tokeni o‘z parol xeshi va siyosat versiyalariga bog‘lanadi. Tasdiqlash formasida o‘sha ro‘yxatdan o‘tish paroli so‘raladi. Email manzilini oldindan kiritgan odamning paroli tasodifan faollashib qolmasligi uchun hisobning faol paroli va rozilik yozuvlari faqat token hamda parol tekshiruvidan keyin saqlanadi. Bir manzil uchun urinishlar limit ichida mustaqil. Birinchi muvaffaqiyatli tasdiqlash boshqa ochiq tokenlarni bekor qiladi.

Xatlardagi tokenlar 32 tasodifiy baytdan tuziladi va bazada SHA-256 xeshi ko‘rinishida saqlanadi. Havola tokenni URL fragmentida olib yuradi. Brauzer GET so‘rovida fragmentni uzatmaydi. Token sarfi va hisob o‘zgarishi bir tranzaksiyada bajariladi. Hisob qatori blokirovkasi bir vaqtning o‘zida kelgan ikki tasdiqlash yoki tiklashdan faqat bittasini o‘tkazadi.

Tiklash so‘rovi bazadan hisob qidirishni cheklangan SMTP navbatidagi ishga topshiradi. Bu hisob mavjudligiga bog‘liq email yoki baza ishlovini HTTP javobidan ajratadi. Navbat jarayon xotirasida va lokal Mailpit bilan ishlaydi. Bazada ochiq token saqlanmaydi. Dev muddatlari tasdiqlash uchun 24 soat, tiklash uchun 30 daqiqa. Production email transporti va amal qilish muddatlari ommaviy muhitga chiqishdan oldin tasdiqlanadi.
