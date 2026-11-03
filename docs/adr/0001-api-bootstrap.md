# Lokal API karkasi

Sana — 2026-yil 3-noyabr

V1.0 reja backend uchun Axum, Tokio, SQLx, PostgreSQL va utoipa’ni belgilaydi. B0 jarayon holati va baza tayyorligini alohida ko‘rsatadi. API ishga tushganda SQLx ulanish havzasini yaratadi. `/health` faqat API jarayonini, `/ready` esa chegaralangan vaqtda `SELECT 1` orqali PostgreSQL bilan aloqani tekshiradi. Baza tiklansa, readiness qayta ishga tushirishsiz tiklanadi. Birinchi zarur migratsiyalar B1’da qo‘shiladi, o‘shanda readiness migratsiya holatini ham tekshiradi.

Bitta umumiy probe DB uzilganda API jarayonining holatini ajratib ko‘rsata olmaydi. PostgreSQL’ga ishga tushishda majburiy ulanish esa liveness va readiness’ni bir xil natijaga bog‘laydi. Alohida probe’lar Compose sinovida database uzilishi va tiklanishini kuzatish imkonini beradi.

OpenAPI endpointi va versiyalanadigan JSON fayli bir xil `utoipa` ta’rifidan yaratiladi. Qo‘lda yuritiladigan OpenAPI fayli marshrut o‘zgarishidan orqada qolishi mumkin. CI generatsiya qilingan kontraktni repodagi fayl va HTTP javobi bilan solishtiradi.
