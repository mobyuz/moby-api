# Moby API

Moby backendida hisoblar, email tasdiqlash va qurilmaga bog‘langan sessiyalar ishlaydi. Rust va Axum HTTP API’ni, Tokio asinxron ishlarni, SQLx esa PostgreSQL ulanishini boshqaradi. OpenAPI kontrakti Rust ta’riflaridan generatsiya qilinadi.

Loyiha GPL-3.0-only litsenziyasi ostida tarqatiladi. Copyright (c) 2026 Moby contributors. [Litsenziya matni](LICENSE)

## Windowsda ishga tushirish

Docker Desktop’da Linux engine ishlayotganini tekshiring. Buyruqlarni `moby-api` papkasida PowerShell’dan bajaring.

```powershell
docker compose up --build --detach --wait
Invoke-RestMethod http://127.0.0.1:8080/health
Invoke-RestMethod http://127.0.0.1:8080/ready
Invoke-RestMethod http://127.0.0.1:8080/openapi.json
```

Build parallelizmini cheklash uchun `docker compose build --build-arg CARGO_BUILD_JOBS=2` buyrug‘idan foydalaning. Keyin `docker compose up --no-build --detach --wait` bilan tayyor image’ni ishga tushiring.

| Endpoint | Javob |
| --- | --- |
| `GET /health` | API ishlayotganida `200` va `{"status":"ok"}` |
| `GET /ready` | PostgreSQL va kerakli migratsiya tayyor bo‘lsa `200`, aks holda `503` |
| `GET /openapi.json` | `200` va OpenAPI 3.1 kontrakti |

API ishga tushishda SQLx migratsiyalarini qo‘llaydi. Mailpit interfeysi `http://127.0.0.1:8025`, SMTP porti `1025`. Compose lokal ishlab chiqish uchun. Ishni tugatgach `docker compose down` bilan servislarni to‘xtating.

### Rust serverini hostda ishga tushirish

Rustup va Windows MSVC build vositalari kerak. Quyidagi buyruqlar bazani va Mailpit’ni konteynerda, API’ni hostda ishga tushiradi.

```powershell
docker compose up --detach --wait postgres mailpit
$env:DATABASE_URL = 'postgres://moby:moby_dev@127.0.0.1:55432/moby?sslmode=disable'
cargo run --locked
```

`DATABASE_URL` majburiy. `MOBY_BIND_ADDR` standart qiymati `127.0.0.1:8080`. Lokal misol `.env.example` faylida. Hostda ishga tushirishda qiymatlar jarayon muhitidan o‘qiladi. HTTP logida server yaratgan `request_id`, marshrut shabloni, status va davomiylik bor.

## Hisobni lokal sinash

```powershell
$body = @{
    email = 'test@example.test'
    password = 'Local test password 2026'
    terms_version = 'dev-1'
    privacy_policy_version = 'dev-1'
} | ConvertTo-Json
Invoke-RestMethod http://127.0.0.1:8080/v1/auth/register -Method Post -ContentType application/json -Body $body
```

Mailpit’da kelgan tasdiqlash havolasini ochib, ro‘yxatdan o‘tishda ishlatilgan parolni kiriting. `POST /v1/auth/password/forgot` `{ "email": "test@example.test" }` qabul qiladi. Yangi xatdagi havolada parolni almashtiring. Forma bilan bir qatorda ikkala amal OpenAPI’da hujjatlangan JSON endpoint orqali ham ishlaydi. API muvaffaqiyatli tasdiqlash va tiklashga `204`, ro‘yxatdan o‘tish va tiklash xati so‘roviga `202` qaytaradi.

Lokal sozlamada tasdiqlash havolasi 24 soat, tiklash havolasi 30 daqiqa amal qiladi. `MOBY_ACCOUNT_BASE_URL` xatlardagi havola manzilini belgilaydi. SMTP transporti lokal Mailpit uchun sozlangan. Xat yuborish navbati jarayon xotirasida turadi. Xat yetib kelmasa, so‘rovni limit doirasida qayta yuborish mumkin.

## Login va sessiyalar

| Endpoint | Kirish ma’lumoti va natija |
| --- | --- |
| `POST /v1/auth/login` | `email`, `password`, ixtiyoriy `installation_id`. Qurilma ID’si berilmasa, Windows qurilma yozuvi yaratiladi. Javobda token juftligi va `installation_id` bor. |
| `POST /v1/auth/refresh` | JSON’da `refresh_token`. Javobda yangi access va refresh tokenlar bor. |
| `POST /v1/auth/logout` | JSON’da `refresh_token`. Sessiya bekor qilinadi va `204` qaytadi. |
| `GET /v1/me` | Bearer access token. Hisob ID’si, email, profil nomi va joriy qurilma ID’si qaytadi. |
| `GET /v1/installations` | Bearer access token. Hisobga tegishli qurilmalar ro‘yxati qaytadi. |
| `POST /v1/installations` | Bearer access token, `name` va ixtiyoriy `app_version`. Windows qurilma yozuvi yaratiladi. |
| `DELETE /v1/installations/{id}` | Bearer access token. Hisobga tegishli qurilma va uning sessiyalari bekor qilinadi. |

Access token Ed25519 bilan imzolanadi va 15 daqiqa amal qiladi. Sessiyaning eng ko‘p muddati birinchi logindan 30 kun. Refresh token har ishlatilganda almashadi. Desktop bitta sessiya uchun refresh so‘rovlarini ketma-ket yuborishi kerak. Qayta ishlatilgan refresh shu sessiyadagi barcha tokenlarni bekor qiladi. Parolni tiklash hisobning barcha sessiyalarini bekor qiladi.

Lokal loopback va Compose uchun vaqtinchalik imzo kaliti yaratiladi. Jarayon qayta ishga tushsa, desktop amaldagi refresh token bilan yangi access token oladi. Doimiy kalit uchun sirlar omboridan `MOBY_AUTH_SIGNING_SEED`ga 32 baytli hex qiymat bering. `moby-api auth-public-key` joriy seed’dan faqat ochiq kalitni chiqaradi. Rotatsiyada oldingi ochiq kalitlarni `MOBY_AUTH_PREVIOUS_PUBLIC_KEYS`ga vergul bilan ajratib kiriting va eski access tokenlar muddati tugagach o‘chiring.

## Profil va roziliklar

Bu endpointlar Bearer access token talab qiladi.

| Endpoint | Natija |
| --- | --- |
| `PATCH /v1/me` | `display_name` matnini saqlaydi. `null` qiymati profil nomini tozalaydi. |
| `POST /v1/me/export` | Hisob, qurilma, sessiya, rozilik va bog‘langan identifikator metadatasini `moby-account-export.json` sifatida qaytaradi. |
| `DELETE /v1/me` | Joriy `password` yoki bog‘langan Google hisobining yangi `code` va `code_verifier` tasdig‘ini tekshiradi. Hisob va unga tegishli yozuvlar asosiy PostgreSQL bazasidan o‘chiriladi, tokenlar kuchini yo‘qotadi. |
| `GET /v1/consents` | Joriy siyosat versiyalari va hisobning rozilik yozuvlarini qaytaradi. |
| `POST /v1/consents` | JSON’da `type`, `version`, `accepted` qabul qiladi. `type` qiymatlari `terms`, `privacy_policy` va `cloud_voice`. |

Rozilik faqat joriy versiyaga beriladi. Saqlangan avvalgi versiyadan ham voz kechish mumkin. Bulutli ovoz roziligi alohida yozuvda saqlanadi va uning versiyasi `MOBY_CLOUD_VOICE_VERSION` orqali belgilanadi. Lokal standart qiymat `dev-1`.

## Google orqali kirish

Google Cloud’da **Web application** turidagi OAuth klientini yarating. Google’da ro‘yxatga olingan qaytish manzilini `MOBY_GOOGLE_REDIRECT_URI`ga aniq kiriting. Lokal misol `http://127.0.0.1:8080/v1/auth/google/callback`. `MOBY_GOOGLE_CLIENT_ID` va `MOBY_GOOGLE_CLIENT_SECRET` qiymatlarini API muhiti orqali bering. Uchta parametr birga yoqiladi. [Google Web OAuth sozlamalari](https://developers.google.com/identity/protocols/oauth2/web-server)

1. Desktop tizim brauzeridan foydalanadi va faqat `127.0.0.1`da tasodifiy bo‘sh portga qisqa muddatli `/oauth/moby` tinglovchisini ochadi. 32 tasodifiy baytdan padding’siz base64url `desktop_state` va alohida PKCE verifier yaratadi.
2. Foydalanuvchi joriy shartlar va maxfiylik siyosatini tasdiqlagach, desktop brauzerda `GET /v1/auth/google/start`ni ochadi. Query parametrlarida `desktop_port`, `desktop_state`, S256 `code_challenge`, `terms_version` va `privacy_policy_version` bor.
3. Google API’ning `/v1/auth/google/callback` endpointiga qaytadi. API desktop tinglovchisiga `state` va bir martalik `code` yuboradi. Desktop kutilgan `state`ni tekshiradi va tinglovchini yopadi. Bekor qilingan yoki yaroqsiz oqim `error` bilan qaytadi.
4. Desktop API’ga `POST /v1/auth/google/exchange` orqali `code`, `code_verifier` va mavjud bo‘lsa `installation_id`ni yuboradi. Javob oddiy Moby token juftligi. Google urinishining muddati 10 daqiqa, desktop kodi 90 soniya amal qiladi.

`account_link_required` qaytsa, mavjud Moby hisobiga email va parol bilan kiring. Shu Google kodi va verifierini joriy Moby paroli bilan `POST /v1/auth/google/link`ga Bearer token orqali yuboring. `204` qaytgach keyingi Google kirishi bog‘langan hisobga sessiya beradi. Google hisobini o‘chirish tasdig‘i uchun yangi oqimdan olingan kodni `/exchange` o‘rniga `DELETE /v1/me`ga yuboring. [Windows loopback callback tamoyili](https://www.rfc-editor.org/rfc/rfc8252.html#section-7.3)

## Agent API

`POST /v1/agent/turn` uchun faol sessiyaning `Authorization: Bearer <access_token>` sarlavhasini va JSON yuboring.

```json
{
  "messages": [{ "kind": "user", "text": "Salom, Mobi" }],
  "available_tools": [],
  "resources": [],
  "language": "uz",
  "script": "latn"
}
```

Javob `text/event-stream` oqimi. Har bir `data:` freymida bitta JSON hodisa keladi. Nol yoki undan ko‘p `speech_delta`dan keyin bitta `completed`, `tool_call` yoki `error` keladi. `completed`dagi `speech_text` yakuniy matn. `tool_call`, `error` yoki terminal hodisasiz uzilish bo‘lsa, desktop vaqtinchalik matnni bekor qiladi. SSE boshlanishidan oldingi xatolar HTTP status va JSON bilan qaytadi.

Mock’da `git status` matni uchun `available_tools` ro‘yxatiga `git_status`ni va `resources` ro‘yxatiga `{"kind":"project","id":"demo-project","display_name":"Demo"}`ni qo‘shing. Desktop `tool_call`ni mahalliy tekshiradi va bajaradi. Keyingi so‘rov tarixiga olingan `{"kind":"tool_call","call":...}`ni va darhol shu `call_id`ga mos `{"kind":"tool_result","result":...}`ni qo‘shing. API tarixni boshidan tekshiradi. So‘rov `user` bilan boshlanadi va `user` yoki mos `tool_result` bilan tugaydi. Tarixdagi vosita va resurslar joriy e’lon qilingan ro‘yxatga mos bo‘lishi kerak.

Limitlar 128 KiB JSON, 64 xabar, 64 resurs, matn yoki vosita natijasi uchun 8 KiB va 15 soniyalik provayder muddati. Mock o‘zbek lotin va kirill, rus hamda ingliz tillarida deterministik javob qaytaradi. Desktop suhbat tarixini saqlaydi. API suhbat va vosita natijalarini so‘rov davomida xotirada ishlatadi. [OpenAPI kontrakti](openapi.json) · [Agent oqimi bo‘yicha ADR](docs/adr/0006-agent-stream.md)

### Kvota va xizmat byudjeti

`GET /v1/usage` faol Bearer sessiyasi uchun qolgan LLM tokenlari, STT soniyalari, TTS belgilarini va keyingi UTC yarim tunini `reset_at` maydonida qaytaradi. Ovoz o‘chirilganda uning qoldiqlari 0. API provayderga murojaat qilishdan oldin foydalanuvchi kvotasi va xizmat byudjetidan yuqori chegarani band qiladi. Hisobot kelganda ortiqcha bandlov bo‘shaydi. Timeout yoki uzilishda sarf noma’lum qolsa, bandlov o‘z davrida hisoblanadi. Kvota yetmasa `429 quota_exceeded`, byudjet yetmasa `503 service_budget_exhausted` qaytadi.

| Muhit o‘zgaruvchisi | Standart | Ma’nosi |
| --- | --- | --- |
| `MOBY_LLM_DAILY_TOKENS` | `250000` | Bir hisobning UTC kunlik LLM kvotasi. Mock matnni deterministik UTF-8 birliklarida hisoblaydi. |
| `MOBY_LLM_DAILY_BUDGET_MICROUSD` | `0` | LLM, STT va TTS uchun umumiy UTC kunlik xarajat chegarasi. |
| `MOBY_LLM_MONTHLY_BUDGET_MICROUSD` | `0` | LLM, STT va TTS uchun umumiy UTC oylik xarajat chegarasi. |
| `MOBY_AGENT_ENABLED` | `true` | Agent turnlarini to‘xtatish uchun `false` qilib API’ni qayta ishga tushiring. |

1 USD = 1 000 000 mikroUSD. Mock xarajati 0. Provayder ketma-ket uch marta xato qilsa, servis yangi agent so‘rovlarini 30 soniyaga to‘xtatadi. So‘ng bitta sinov so‘rovi o‘tadi va muvaffaqiyatli javob kelganda trafik ochiladi. [Kvota va bandlov ADR’i](docs/adr/0007-agent-usage.md)

## Ovoz API

Har ikkala endpoint faol Bearer sessiyasi va joriy `cloud_voice` roziligini talab qiladi. Rozilik versiyasini `GET /v1/consents`dan oling. `POST /v1/consents`ga `{"type":"cloud_voice","version":"dev-1","accepted":true}` yuborish lokal standart versiya uchun rozilik beradi. Rozilik bekor qilinganda yoki versiya eskirganda yangi ovoz so‘rovi `403 voice_consent_required` oladi.

| Endpoint | So‘rov | Javob |
| --- | --- | --- |
| `POST /v1/voice/stt` | `audio/wav` tanasida mono 16 kHz, 16-bit PCM. 30 soniyagacha audio va 4 KiB WAV sarlavha hamda metadata. `language` va `script` query parametrlari ixtiyoriy. | `text`, `language`, `script` va 0–1 oralig‘idagi `confidence` bilan JSON. |
| `POST /v1/voice/tts` | `application/json` tanasida 1–500 Unicode belgili `text` hamda ixtiyoriy `language` va `script`. | Mono 16 kHz, 16-bit PCM `audio/wav`. |

Til va yozuv juftlari `uz/latn`, `uz/cyrl`, `ru/cyrl` va `en/latn`. Standart juft `uz/latn`. STT kvotasi audio davomiyligini yuqoriga butun soniyaga yaxlitlaydi. TTS kvotasi Unicode belgilarini sanaydi. Provayder muddati 15 soniya. Audio va matn so‘rov davomida xotirada ishlatiladi.

Dev Compose ovoz mockini standart yoqadi. Hostda `MOBY_VOICE_ENABLED=true` bilan yoqing. `MOBY_STT_DAILY_SECONDS` va `MOBY_TTS_DAILY_CHARS` standartlari mos ravishda 300 va 5000. Mock STT tilga mos tayyor test matnini, mock TTS esa WAV sinov signalini beradi. [Ovoz API qarorlari](docs/adr/0008-voice-api.md)

## Tekshiruvlar

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
docker compose config --quiet
```

OpenAPI yangilanganda PowerShell 7.4 yoki yangiroq versiyasida faylni qayta generatsiya qiling.

```powershell
cargo run --locked --quiet -- openapi > openapi.json
cargo test --locked
```

Test OpenAPI faylini generatsiya qilingan kontrakt va HTTP javobi bilan solishtiradi. CI’dagi Compose sinovi PostgreSQL o‘chirilganda `/ready` 503 berishini va PostgreSQL qayta ishga tushgach 200 ga qaytishini tekshiradi.

Toza lokal Compose loyihasi ishga tushganida `pwsh -File scripts/smoke-accounts.ps1 -Project <loyiha-nomi>` bilan hisob oqimini sinang. Skript sintetik test hisoblarini yaratadi. Migratsiya yoki SQL o‘zgarganda PostgreSQL tayyor bo‘lishi kerak. SQLx CLI 0.8.6 bilan `cargo sqlx prepare -- --locked` metadata fayllarini yangilaydi. CI `cargo sqlx prepare --check -- --locked` bilan ularni tekshiradi.

`pwsh -File scripts/smoke-sessions.ps1 -Project <loyiha-nomi>` sessiyalar, profil, eksport, rozilik va hisobni o‘chirishni ikkita sintetik hisob bilan sinaydi.

`MOBY_GOOGLE_TEST_DATABASE_URL`ni ajratilgan PostgreSQL bazasiga sozlab, `cargo test --locked --lib google::tests::google_round_trip -- --ignored`ni bajaring. Test Google token va JWKS endpointlarini lokal simulyatsiya qiladi.

`pwsh -File scripts/smoke-agent.ps1 -Project <loyiha-nomi>` tasdiqlangan hisob, SSE javobi, `git_status` oqimi, kvota hisoblanishi va sintetik suhbat mazmuni saqlanmasligini tekshiradi.

`MOBY_USAGE_TEST_DATABASE_URL`ni ajratilgan PostgreSQL bazasiga sozlab, `cargo test --locked --lib usage_ -- --ignored`ni bajaring. Testlar parallel kvota va byudjet bandlovlari, takroriy yakunlash, timeout, uzilish va qayta ulanishni tekshiradi.

`MOBY_VOICE_TEST_DATABASE_URL` shu turdagi ajratilgan bazaga sozlanganda `cargo test --locked --lib voice::tests::voice_consent_quota_and_locale_round_trip -- --ignored` ovoz endpointlari, to‘rt til va yozuv jufti, rozilikning bekor qilinishi hamda kvotani tekshiradi.
