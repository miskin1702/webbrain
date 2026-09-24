# WebBrain Workspace Bridge V1 -> OMP SDK Coding Worker V2
## Kusursuz Migrasyon Gorevi / Uygulama Sozlesmesi

> Bu dosya, mevcut calisan WebBrain Local Workspace Bridge V1 mimarisini kontrollu, geri alinabilir ve performans odakli bicimde OMP SDK tabanli V2 mimarisine tasimak icin uygulanacak tek kaynakli gorev spesifikasyonudur.
>
> Bu bir fikir dokumani degildir. Uygulama ajani once mevcut kodu ve repolari kesfedecek, sonra bu sozlesmeye gore migrasyonu uctan uca gerceklestirecek, test edecek, benchmark yapacak, upstream-update provasini yapacak ve GitHub branch'ine push edecektir.

---

# 0. ANA NIYET

Mevcut sistem calisiyor. Onu sifirdan yeniden yazmak veya bir anda kaldirmak istemiyoruz.

Bugunku V1 mimarisinde WebBrain icindeki agent, yerel codebase islemlerini dusuk seviyeli workspace tool'lari araciligiyla Rust daemon'a yaptiriyor:

```text
WebBrain Agent
    |
    | workspace_search_code / read / apply_patch / git_diff / run_command
    v
Chrome Extension
    |
    v
Offscreen WebSocket
    |
    v
Rust Workspace Daemon
    |
    +-- filesystem
    +-- search
    +-- patch
    +-- revision/conflict
    +-- watcher
    +-- git
    +-- command runner
    v
Workspace
```

Bu mimari teknik olarak calismaktadir; ancak coding-agent tarafinda OMP'nin zaten sahip oldugu yetenekleri Rust icinde tekrar implemente etmektedir.

Yeni hedefimiz:

1. **WebBrain browser uzmani olarak kalacak.**
   - DOM, click, type, console, network, page state, reload ve browser dogrulamasi WebBrain/extension tarafinda kalacak.
   - Browser islemleri OMP uzerinden dolastirilmayacak.

2. **OMP SDK coding uzmani olacak.**
   - Codebase arama, dosya okuma, edit, LSP, test, shell/git ve coding agent loop OMP SDK tarafinda gerceklesecek.
   - OMP bir browser agent'a donusturulmeyecek.

3. **Mevcut OMP SDK uygulamasi yeniden kullanilacak.**
   - Kullanici zaten yerelde OMP SDK kullanan ve OMP provider/model katmanini API olarak sunan bir uygulamaya sahip.
   - Yeni coding service, bu mevcut uygulamanin icine modul olarak eklenecek.
   - Mevcut provider API davranisi bozulmayacak.

4. **Rust nihai mimariden cikarilacak.**
   - Ancak Rust V1 ilk gunden silinmeyecek.
   - V2 once paralel calisacak.
   - Parity + guvenlik + performans + rollback testleri gecmeden Rust kaldirilmayacak.

5. **Iki ayri hizli yol olacak.**

```text
Browser fast path:
WebBrain -> Chrome

Coding fast path:
WebBrain -> localhost WebSocket -> mevcut OMP SDK host -> AgentSession -> Workspace
```

6. WebBrain ile OMP arasinda **her tool ciktisi tasinmayacak**.
   - WebBrain OMP'ye yuksek seviyeli coding task devredecek.
   - OMP kendi local tool loop'unu kendi icinde calistiracak.
   - WebBrain'e yalnizca progress, sonuc, degisen dosyalar, test sonucu ve browser verification istegi donecek.

Bu tasarimin ana amaci **hiz, dusuk latency, az context tasimasi, az tekrar kod ve net sorumluluk ayrimi**dir.

---

# 1. MEVCUT V1 DURUMUNU KABUL ET VE KORU

Mevcut uygulama raporuna gore V1'de asagidaki bilesenler bulunmaktadir. Bunlari repo gercegiyle tekrar dogrula; rapora koru korune guvenme.

## Rust daemon

Beklenen dizin:

```text
workspace-bridge/
```

Beklenen sorumluluklar:

```text
server.rs
protocol.rs
auth.rs
paths.rs
files.rs
patch.rs
search.rs
watcher.rs
git.rs
command.rs
session.rs
```

## Chrome/WebBrain entegrasyonu

Beklenen dosyalar:

```text
src/chrome/src/offscreen/workspace-bridge.js
src/chrome/src/workspace-runs.js
src/chrome/src/agent/workspace-tools.js
src/chrome/src/agent/tools.js
src/chrome/src/agent/permission-gate.js
src/chrome/src/agent/agent.js
src/chrome/src/background.js
src/chrome/src/ui/settings.*
src/chrome/src/ui/sidepanel.*
```

Firefox kopyalari mevcut olabilir. Ana hedef Chrome/Chromium + Windows 11'dir. Firefox desteğini migrasyonun kritik yoluna sokma; mevcut kodu kirmadan koru, fakat yeni mimariyi once Chrome'da dogrula.

## V1 testleri

Beklenenler:

```text
test/workspace/e2e-coding-loop.mjs
test/workspace/workspace-tools.test.mjs
test/workspace/workspace-bridge.test.mjs
test/security/injection-corpus.mjs
```

V1 testlerini migrasyon sirasinda silme. Bunlar regression oracle olarak kullanilacak.

---

# 2. HEDEF V2 MIMARISI

Nihai hedef:

```text
                                  USER
                                    |
                                    v
                         +---------------------+
                         |  WebBrain Sidepanel |
                         +----------+----------+
                                    |
                   +----------------+----------------+
                   |                                 |
                   v                                 v
          BROWSER FAST PATH                  CODING HANDOFF
                   |                                 |
                   v                                 v
          WebBrain Browser Agent            localhost WebSocket
                   |                                 |
          DOM/CDP/Network/Console                    v
                   |                       +---------------------+
                   v                       | Existing OMP SDK App|
                Chrome                     |                     |
                                           | Provider Plane      |
                                           |   (existing)        |
                                           |                     |
                                           | Agent Plane         |
                                           |   (new WebBrain     |
                                           |    coding service)  |
                                           +----------+----------+
                                                      |
                                                      v
                                               OMP AgentSession
                                                      |
                                          read / grep / glob / edit
                                             lsp / bash / git
                                                      |
                                                      v
                                                   Workspace
```

Nihai durumda:

```text
Rust daemon               -> kaldirilmis
OMP RPC child process     -> yok
OMP executable spawn      -> yok
OMP SDK                    -> ayni process icinde
WebBrain browser tools    -> WebBrain'de
Coding tools              -> OMP SDK session'da
Provider API              -> mevcut uygulamada aynen korunmus
```

---

# 3. EN ONEMLI MIMARI KURAL: PROVIDER PLANE VE AGENT PLANE AYRI

Mevcut OMP SDK uygulamasi provider/model katmanini API olarak sunuyor. Bu mevcut davranis korunacak.

Yeni coding entegrasyonunu provider endpointlerinin icine gomerek karistirma.

Hedef:

```text
                    EXISTING OMP SDK HOST
                           |
               +-----------+-----------+
               |                       |
               v                       v
        PROVIDER PLANE             AGENT PLANE
        existing API               new module
               |                       |
        request/response        stateful coding sessions
               |                       |
         model/provider          AgentSession per workspace/task
```

## Provider Plane

- Mevcut API endpointleri korunacak.
- Mevcut istemcilerin contract'i degismeyecek.
- Mevcut auth/model/provider davranisi regression testleriyle korunacak.
- Coding task'lari mevcut OpenAI-compatible/provider endpointine dolastirilmamali.

## Agent Plane

Yeni modul:

```text
webbrain/
  websocket.ts
  protocol.ts
  coding-session.ts
  workspace-manager.ts
  task-manager.ts
  auth.ts
  events.ts
  permissions.ts
```

Gercek isimleri mevcut repo stiline uyarla. Bu liste contract degil, sorumluluk siniridir.

Agent Plane, provider katmanindan yalnizca paylasilmasi mantikli servisleri kullanabilir:

```text
AuthStorage
ModelRegistry
provider configuration
logging/telemetry infrastructure (uygunsa)
```

Fakat sunlari paylasmamalidir:

```text
AgentSession instance
per-task conversation state
workspace cwd
coding task lifecycle
mutable coding session state
```

---

# 4. OMP SDK GERCEKLERI - UYGULAMA ONCESI DOGRU SURUMU DOGRULA

Migrasyon basinda mevcut projedeki `@oh-my-pi/pi-coding-agent` surumunu tespit et.

Asagidaki davranislar guncel upstream SDK dokumantasyonunda vardir, ancak implementasyon yaparken **kurulu/pinlenen surumun tiplerini ve gercek API'sini kaynak koddan dogrula**:

- `createAgentSession()` in-process embed surface'tir.
- SDK dogrudan agent state, event streaming, tool wiring ve session control saglar.
- `toolNames` tek basina allowlist DEGILDIR.
- Gercek tool kisitlamasi icin `restrictToolNames: true` kullanilmalidir.
- Restricted session ambient MCP, extensions, custom commands ve LSP'yi varsayilan olarak kapatir.
- LSP gerekiyorsa ilgili option/tool kombinasyonu explicit olarak acilmalidir.
- `session.subscribe(...)` typed event stream saglar.
- `session.prompt(...)`, `steer(...)`, `followUp(...)`, `abort()` vardir.
- Session bittiginde `await session.dispose()` zorunludur.
- Birden fazla concurrent top-level session varsa private `AgentRegistry` kullanilmalidir.
- `authStorage` ve `modelRegistry` birlikte veriliyorsa ayni AuthStorage instance'ina bagli olmalidir.
- `getActiveToolNames()` ile aktif tool set runtime'da dogrulanabilir.

Referanslar:

- https://github.com/can1357/oh-my-pi/blob/main/docs/sdk.md
- https://github.com/can1357/oh-my-pi/blob/main/docs/tools/edit.md
- https://github.com/can1357/oh-my-pi/blob/main/docs/custom-tools.md

Bu referanslari implementation zamani tekrar kontrol et; API'yi tahmin ederek kod yazma.

---

# 5. WEBBRAIN VE OMP'NIN SORUMLULUKLARI

Bu tablo V2'nin temel kontratidir.

| Sorumluluk | WebBrain | OMP SDK Host |
|---|---:|---:|
| Browser DOM okumak | EVET | HAYIR |
| Click/type/navigation | EVET | HAYIR |
| Browser network inspect | EVET | HAYIR |
| Browser console inspect | EVET | HAYIR |
| Browser screenshot/state | EVET | HAYIR |
| Problemi browserda teshis etmek | EVET | destekleyici veri alir |
| Codebase aramak | HAYIR (V2 normal yol) | EVET |
| Dosya okumak | HAYIR (V2 normal yol) | EVET |
| Kod edit etmek | HAYIR (V2 normal yol) | EVET |
| LSP/references/diagnostics | HAYIR | EVET |
| Test/lint/build | HAYIR | EVET |
| git diff/status | HAYIR | EVET |
| Coding agent loop | HAYIR | EVET |
| Browser verification | EVET | yalnizca ister |
| Kullaniciya browser UI gostermek | EVET | HAYIR |
| Provider/model credentials | kullanmaz | mevcut host altyapisi |

V1 compatibility modu haric WebBrain'e tekrar `read_file`, `apply_patch`, `run_command` gibi coding primitive'leri vermek yasaktir.

---

# 6. HIZ PRENSIBI: IKI FAST PATH, MINIMUM HANDOFF

## Browser fast path

```text
WebBrain -> Chrome
```

Browser eylemi icin:

```text
WebBrain -> OMP -> host -> browser
```

gibi bir yol KURMA.

## Coding fast path

```text
WebBrain -> OMP SDK host -> AgentSession -> workspace
```

Coding islerini:

```text
WebBrain -> Rust -> filesystem
```

uzerinden yapma.

## Handoff yalnizca gorev seviyesinde

Yanlis:

```text
WebBrain -> OMP: grep yap
OMP -> WebBrain: grep sonucu
WebBrain -> OMP: read yap
OMP -> WebBrain: read sonucu
WebBrain -> OMP: edit yap
```

Dogru:

```text
WebBrain -> OMP:
"POST /api/login 500 veriyor. normalizeUser TypeError gozledim.
Yerel codebase'de sebebi bul, duzelt, ilgili testleri calistir ve sonuc bildir."

OMP kendi icinde:
grep -> read -> lsp -> edit -> test -> edit -> test

OMP -> WebBrain:
"2 dosya degisti, 18 test gecti. Browserda login akisini tekrar dogrula."
```

---

# 7. WEBBRAIN <-> OMP AGENT PLANE PROTOKOLU

V1'in dusuk seviyeli workspace JSON-RPC benzeri API'si V2'nin public contract'i OLMAYACAK.

Yeni protokol task-oriented olacak.

Wire format JSON over localhost WebSocket olabilir. Mevcut OMP SDK uygulamasinin hali hazirda bir HTTP/WebSocket sunucusu varsa yeni endpointi mevcut server'a ekle. Gereksiz ikinci server baslatma.

Onerilen namespace:

```text
/webbrain/coding
```

Gercek route mevcut app konvansiyonuna gore secilebilir.

## Client -> Host mesajlari

Minimum:

```text
hello
workspace.open
coding.start
coding.steer
coding.follow_up
coding.abort
coding.status
session.close
verification.result
```

## Host -> Client eventleri

Minimum:

```text
hello.ok
workspace.opened
coding.started
coding.progress
coding.tool_activity
coding.changed_files
coding.verification_requested
coding.completed
coding.failed
coding.aborted
session.closed
host.error
```

### coding.start ornegi

```json
{
  "v": 2,
  "id": "req-123",
  "type": "coding.start",
  "workspaceId": "ws-abc",
  "task": {
    "summary": "Fix login 500 error",
    "instructions": "Find the root cause, make the smallest correct code change, run relevant tests, and report changed files.",
    "browserObservations": {
      "url": "http://localhost:3000/login",
      "request": "POST /api/login",
      "status": 500,
      "console": ["TypeError: normalizeUser ..."]
    },
    "verificationGoal": {
      "description": "Login submit should complete without server error"
    }
  }
}
```

### coding.completed ornegi

```json
{
  "v": 2,
  "type": "coding.completed",
  "taskId": "task-456",
  "status": "completed",
  "summary": "Handled empty user normalization before persistence.",
  "changedFiles": [
    "src/auth/user.ts",
    "tests/auth.test.ts"
  ],
  "checks": [
    {
      "command": "npm test -- auth.test.ts",
      "exitCode": 0,
      "summary": "18 passed"
    }
  ],
  "verificationRequest": {
    "kind": "browser",
    "steps": [
      "reload-current-page",
      "submit-login-form",
      "confirm-/api/login-is-not-5xx",
      "confirm-success-ui-state"
    ]
  }
}
```

Bu JSON orneklerini literal schema olarak koru korune uygulama. Repo'da Zod/TypeBox/JSON Schema gibi mevcut bir schema sistemi varsa onu kullan.

---

# 8. BROWSER VERIFICATION DONGUSU

Bu sistemin ana farki coding tamamlandiktan sonra browser dogrulamasinin OMP tarafinda degil WebBrain tarafinda hizla yapilmasidir.

Tam dongu:

```text
1. User -> WebBrain
2. WebBrain browserda problemi gozler
3. WebBrain kompakt coding handoff olusturur
4. OMP SDK coding loop'u calisir
5. OMP test/build/lint ile yerelde dogrular
6. OMP WebBrain'e verification request dondurur
7. WebBrain browseri reload/refresh eder
8. WebBrain ayni akis uzerinde hizli browser verification yapar
9a. Basarili -> gorev tamamlanir
9b. Basarisiz -> yeni browser observation OMP'ye follow-up/steer olarak gider
10. OMP sadece gerekli coding duzeltmesini yapar
11. WebBrain tekrar browserda dogrular
```

OMP'ye ilk fazda `browser`, `computer`, MCP browser tool'u veya WebBrain host browser tool'u VERME.

Bu karar performans amaclidir.

---

# 9. OMP CODING SESSION PROFILI

WebBrain icin genel amacli devasa OMP session baslatma.

Hedef: dar ve deterministik coding worker.

Baslangic tool seti:

```text
read
grep
glob
edit
lsp
bash
```

`write` yeni dosya olusturma ihtiyaci icin gerekli olabilir. Uygulama ajani OMP'nin kurulu surumunde `edit` ile create davranisini ve `write` ihtiyacini dogrulasin. Gerekliyse:

```text
read
grep
glob
edit
write
lsp
bash
```

kullan.

Kritik:

```ts
toolNames: [...],
restrictToolNames: true
```

olmalidir.

Sadece `toolNames` vermek allowlist DEGILDIR.

Ayrica:

```text
enableMCP: false
```

kullan veya kurulu SDK surumundeki esdeger explicit MCP-off davranisini dogrula.

Ambient extension/skills/custom commands WebBrain coding worker'a varsayilan olarak yuklenmemeli.

Runtime sonrasinda:

```text
session.getActiveToolNames()
```

ile aktif tool listesini kontrol et.

Beklenmeyen tool varsa session'i production-ready sayma.

## LSP

LSP faydali ama startup maliyeti kontrol edilmeli.

- Lazy LSP tercih et.
- Server'lari session startup'ta gereksiz yere warm etme.
- Ilk `lsp` veya edit ihtiyacinda baslatilmasi tercih edilir.
- Kullanilan OMP SDK surumunde restricted session + LSP semantics'i test ile dogrula.

---

# 10. SESSION MODELI

Provider API request state'i ile coding session state'ini paylastirma.

Her aktif WebBrain coding workspace/task icin kontrollu AgentSession lifecycle olustur.

Baslangic secimi:

- Workspace basina uzun omurlu session + task turn'leri tercih edilebilir.
- Ancak once mevcut OMP SDK app yapisini incele.
- Memory/context leak veya stale task context riski varsa task-bazli session daha dogru olabilir.

Bu secim performans + context kalitesi ile olculmeli.

## Tavsiye edilen ilk model

```text
1 workspace connection
    -> 1 active coding AgentSession
    -> ard arda coding.start / follow-up turn'leri
    -> explicit session.close veya workspace degisince dispose
```

Avantaj:

- repo context tekrar tekrar sifirdan kurulmaz
- model onceki duzeltmeyi bilir
- browser verification failure follow-up olarak ucuzdur

Risk:

- cok uzun session context sisirebilir

Bu nedenle compaction/retry gibi mevcut OMP mekanizmalari korunabilir, ancak ekstra memory/tool discovery acmayi gerektirmez.

## Concurrent session

Birden fazla workspace veya eszamanli top-level session desteklenecekse her session icin private `AgentRegistry` kullan.

Global default registry ile birden fazla `Main` agent cakismasina izin verme.

## Session shutdown

Kapanista:

```text
abort (gerekiyorsa)
beginDispose (wrapper ihtiyaci varsa)
disconnect WebSocket refs
await session.dispose()
```

OOM, leaked watcher, leaked LSP, leaked subprocess birakma.

---

# 11. MEVCUT AUTH / MODEL / PROVIDER ALTYAPISINI YENIDEN KULLAN

Mevcut OMP SDK uygulamasi provider layer'i zaten yonetiyorsa yeni coding session'in ayrica credential dosyasi kesfetmesine gerek kalmayabilir.

Tercih:

```text
existingAuthStorage
existingModelRegistry
       |
       +-- Provider Plane
       |
       +-- WebBrain Agent Plane
```

Ancak:

- `ModelRegistry` ayni `AuthStorage` instance'ina bagli olmali.
- Provider request ile AgentSession ayni mutable conversation state'i kullanmamali.
- Coding worker icin hangi model kullanilacagi explicit config ile belirlenebilmeli.
- Mevcut provider API'nin default model secimini sessizce degistirme.

Config ornegi:

```text
webbrainCoding.enabled=true
webbrainCoding.model=<optional explicit model/pattern>
webbrainCoding.thinkingLevel=<configurable>
webbrainCoding.maxConcurrentSessions=1
webbrainCoding.toolProfile=fast-coding
```

Gercek config sistemine uyarla.

---

# 12. WEBBRAIN'DEKI TOOL MODELINI DEGISTIR

V1 WebBrain agentina su dusuk seviyeli tool'lari sunuyordu:

```text
workspace_status
workspace_search_code
workspace_read_file
workspace_read_range
workspace_apply_patch
workspace_git_diff
workspace_run_command
```

V2'de WebBrain agent coding loop'u kendisi yapmayacak.

Yeni tool yuzeyi mumkun oldugunca kucuk olmali.

Oneri:

```text
coding_delegate
coding_steer
coding_status
coding_abort
```

Gerekirse `coding_follow_up` eklenebilir.

## coding_delegate

WebBrain browser teshisini kompakt bir coding task'a donusturur.

Model-facing description, OMP'nin dosya tool'larini taklit etmemeli.

Ornek niyet:

> Delegate a local codebase implementation/debugging task to the connected OMP coding worker. Include only browser observations relevant to the code problem. The worker owns code search, reading, editing, local tests, git inspection, and implementation iteration. Do not micromanage individual file operations.

## coding_status

Sadece task state/progress icin.

## coding_steer

Calisan coding task'a kullanicidan veya browserdan yeni kritik bilgi iletmek icin.

## coding_abort

Task iptali.

---

# 13. WEBBRAIN AGENT PROMPT / ORKESTRASYON KURALLARI

WebBrain system prompt / agent instruction katmanina su davranis eklenmeli:

1. Browser problemi browser tool'lariyla kendin teshis et.
2. Coding gerektiginde `coding_delegate` kullan.
3. OMP worker'a grep/read/edit gibi mikro-komutlar verme.
4. Browserdan elde ettigin yalnizca coding-relevant kanitlari handoff'a ekle.
5. OMP tamamlayinca browser verification istegini WebBrain kendi browser tools'lariyla uygulasin.
6. Verification basarisizsa yeni sonucu `coding_steer` veya follow-up ile geri ver.
7. Browser verification tamamlanmadan kullaniciya "duzeldi" deme.
8. Local tests gecti diye browser davranisinin duzeldigini varsayma.
9. Browser sonucu olumlu diye local test failure'i yok sayma.
10. Iki uzman alanin birbirinin isini gereksiz yere tekrar etmesini engelle.

---

# 14. CONTEXT VE TOKEN VERIMLILIGI

Bu migrasyonun temel performans hedeflerinden biri iki agent arasinda context tasimasini azaltmaktir.

## WebBrain -> OMP

Gonder:

```text
user goal
relevant URL/route
HTTP method/status
compact network error
compact console stack/error
reproduction steps
expected browser behavior
constraints
```

Gonderme:

```text
full DOM dump
full page HTML
unrelated network log
entire console history
huge screenshots unless essential
WebBrain'in tum conversation transcript'i
```

## OMP -> WebBrain

Gonder:

```text
short implementation summary
changed file paths
check/test summaries
important warnings
browser verification request
fatal blockers
```

Varsayilan olarak gonderme:

```text
full grep output
full source files
full git diff
all internal tool results
OMP hidden reasoning
full test stdout if unnecessary
```

Detay UI'da istege bagli expand edilebilir, fakat agent context'e otomatik basilmamali.

---

# 15. PROGRESS EVENTLERI

Kullanici coding task calisirken WebBrain sidepanel'de gorunur ilerleme olmali.

Ancak her token/event UI'yi bogmamalidir.

Normalize edilen progress eventleri:

```text
queued
starting
searching_code
reading_code
editing
running_checks
waiting_for_model
needs_input
ready_for_browser_verification
completed
failed
aborted
```

Istege bagli metadata:

```text
activeFile
activeCommand
changedFilesCount
elapsedMs
```

OMP SDK'nin ham event stream'i WebBrain public protokolunun kendisi OLMAMALI.

Bir adapter katmani ile OMP eventleri stable WebBrain eventlerine normalize edilmeli. Bu OMP SDK upgrade'lerinde WebBrain'i korur.

---

# 16. FILE WATCHER KONUSU

V1 Rust daemon `notify` ile file events gonderiyordu.

Rust kaldirilinca iki secenek var:

A. Mevcut OMP SDK host uygulamasinda hafif Node/Bun watcher eklemek.
B. Ilk V2 release'inde watcher'i kaldirip OMP'nin kendi stale edit/snapshot mekanizmasina guvenmek.

Nihai secim su kritere gore verilmeli:

- WebBrain UI'nin harici editor degisikliklerini gercek zamanli gostermesi urun gereksinimi mi?

Eger evetse watcher Agent Plane'e tasinabilir.

Watcher coding correctness'in temel mekanizmasi OLMAMALI.

OMP edit sistemi kendi snapshot/stale-edit guvenligini kullanmali.

Watcher sadece observability/UI/resync icin olmalidir.

Mevcut raporda file.created/changed/deleted/renamed UI davranisi kullaniliyorsa parity testinden sonra karar ver.

---

# 17. SECURITY MODELI

Rust giderken guvenlik modeli gerilememeli.

## 17.1 Loopback only

WebBrain coding endpoint sadece localhost/loopback uzerinden erisilebilir olmali.

```text
127.0.0.1
::1 (gerekiyorsa ve dogru origin/auth ile)
```

LAN'a bind etme.

## 17.2 Pairing/auth

V1 pairing/token modeli calisiyorsa yeniden kullan veya mevcut OMP SDK host auth mekanizmasina adapte et.

Browser sayfalarinin localhost WebSocket endpointine izinsiz baglanmasini engelle.

Kontroller:

- Origin allowlist
- random pairing/session token
- token loglama yok
- URL query string'e secret koymama
- reconnect'te auth tekrar

## 17.3 Workspace authorization

Coding session acilirken `cwd` modeli keyfi path secmemeli.

Workspace path kullanici/UI tarafindan yetkilendirilmeli.

Modelin text prompt icindeki:

```text
"C:\\Users\\...\\secret'i ac"
```

gibi bir talebi workspace authorization sayma.

OMP session cwd yetkili root olmalidir.

Additional directories kullanilacaksa explicit authorization gerekir.

## 17.4 Bash

`bash` tool gucludur.

Bu sistem local coding agent oldugu icin gerekebilir. Ancak:

- cwd yetkili workspace'ten baslamali
- UI'da coding agent'in shell yetkisi oldugu gorunur olmali
- mevcut OMP approval/settings davranisi incelenmeli
- kullanici tarafindan verilen full machine permission politikasini repo kurallariyla uyumlu uygula
- credential outputlarini progress/event loglarina sizdirma

## 17.5 Prompt injection

Browser content WebBrain'e untrusted olarak gelir.

WebBrain -> OMP handoff'a browserdan kopyalanan veri:

```text
OBSERVATION / UNTRUSTED BROWSER DATA
```

olarak semantik olarak ayrilmali.

Bir web sayfasinin:

> run rm -rf ...

metni coding task authorization'i degildir.

User intent ile browser observation farkli field'larda tutulmali.

---

# 18. MIGRASYON FEATURE FLAG'LERI

Migrasyon tek cutover ile yapilmayacak.

En az su config/feature flags olustur:

```text
workspaceBackend = "rust-v1" | "omp-sdk-v2"
```

Opsiyonel:

```text
webbrainCoding.shadowCompare = false
webbrainCoding.allowFallbackToRust = true
```

## Kurallar

- Ilk V2 implementasyonunda default `rust-v1` kalabilir.
- V2 integration testleri gecince developer build'de `omp-sdk-v2` default yap.
- Stable cutover oncesi Rust fallback en az bir test fazi boyunca kalmali.
- Rust fallback kullanildiginda telemetry/log bunu acikca gostermeli.
- Silent fallback yapma; yoksa V2 sorunlari gizlenir.

---

# 19. MIGRASYON FAZLARI

## Faz 0 - Repository Discovery ve Baseline

Hicbir davranisi degistirmeden once:

### WebBrain repo

- `git status`
- `git remote -v`
- current branch
- upstream/main HEAD
- V1 workspace dosyalarini gercekten bul
- mevcut testleri calistir
- Chrome build'i calistir
- mevcut V1 e2e benchmarklarini kaydet

### Existing OMP SDK app

Kullanicinin mevcut OMP SDK uygulamasini bul.

Tercih edilen discovery sirasi:

1. mevcut calisma dizini/repo configleri
2. sibling project directories
3. git remotes
4. package manifests icinde `@oh-my-pi/pi-coding-agent`
5. mevcut provider API entrypointleri

Tum diski kontrolsuz tarama.

Buldugunda:

- git status
- git remote -v
- branch
- package manager/runtime
- OMP SDK version
- provider server entrypoint
- AuthStorage/ModelRegistry lifecycle
- server transport (HTTP/WS)
- tests
- build commands

Baseline'i `MIGRATION_BASELINE.md` veya repo docs icinde kaydet.

### Baseline zorunlu testler

- V1 Rust tests
- V1 extension tests
- WebBrain Chrome build
- Existing OMP provider app tests
- Provider API smoke test

Baseline kirmiziysa bunu migrasyon hatasi diye saklama. Once kaydet; ilgili baseline problemi migrasyonla baglantili degilse not dus.

---

## Faz 1 - Git ve Branch Hazirligi

Her repoda `main`/default branch'i dogrudan degistirme.

WebBrain:

```text
feature/omp-sdk-coding-migration
```

OMP SDK app:

```text
feature/webbrain-coding-service
```

veya repo standardina uygun esdeger branch.

Kullanicinin Windows makinesindeki mevcut `git`/`gh` auth'ini kullan.

- PAT isteme
- SSH private key isteme
- credential loglama
- force push yapma
- main'e otomatik merge yapma

Repo kullanicinin fork'uysa origin/upstream modelini koru.

---

## Faz 2 - OMP SDK App Icinde Agent Plane Skeleton

Provider davranisina dokunmadan yeni modulu olustur.

Minimum deliverable:

```text
WebSocket endpoint opens
client auth works
workspace.open works
health/status works
no AgentSession yet or minimal smoke session
provider API regression green
```

Bu faz sonunda WebBrain'e baglama zorunlu degil.

---

## Faz 3 - Restricted OMP AgentSession

Coding session manager implement et.

Zorunlu kontroller:

1. explicit cwd
2. shared AuthStorage/ModelRegistry wiring uygun sekilde
3. private AgentRegistry per concurrent top-level session
4. restricted tool set
5. MCP off
6. ambient extension/custom command discovery off veya restricted semantics ile gercekten etkisiz
7. lazy LSP
8. subscription/event adapter
9. abort
10. dispose

Bir startup self-test ekle:

```text
expectedActiveTools == actualActiveTools
```

Tool profile drift ederse warning/fail policy belirle.

Test task:

> sample fixture'da belirli bir fonksiyonu bul, kucuk edit yap, testi calistir.

Bu noktada Chrome yok.

---

## Faz 4 - Stable Coding Service Protocol

OMP ham eventlerini direkt WebBrain'e gecirme.

Adapter yaz:

```text
OMP AgentSessionEvent
        |
        v
CodingEventNormalizer
        |
        v
WebBrain protocol v2
```

Unit test:

- message delta -> progress
- tool start -> normalized activity
- edit -> changed file tracker
- bash/test -> check summary
- agent_end isTerminal=false -> COMPLETED SAYMA
- terminal agent_end -> finalization
- exception -> coding.failed
- abort -> coding.aborted

Guncel OMP SDK'de `agent_end.isTerminal === false` olabilmektedir; true terminal settle beklenmelidir.

---

## Faz 5 - WebBrain V2 Client

Mevcut Rust offscreen transportunu hemen silme.

Yeni transport/client ekle:

```text
coding-client-v2
```

Mumkunse mevcut offscreen WebSocket altyapisinin generic kisimlarini yeniden kullan; fakat Rust V1 protocol kodunu V2 ile birbirine dolama.

Hedef abstraction:

```text
CodingBackend
  startTask()
  steerTask()
  abortTask()
  getStatus()
  closeSession()
```

Implementasyonlar:

```text
RustWorkspaceBackendV1   (temporary)
OmpSdkCodingBackendV2    (target)
```

V1 dusuk seviyeli tool contract'i ile V2 task-level contract'i ayni interface olmaya zorlanmamali. Gerekirse higher-level orchestration adapter kur.

---

## Faz 6 - WebBrain Agent Handoff Tools

Yeni minimal WebBrain tool'larini ekle:

```text
coding_delegate
coding_steer
coding_status
coding_abort
```

V2 modunda eski:

```text
workspace_search_code
workspace_read_file
workspace_read_range
workspace_apply_patch
workspace_git_diff
workspace_run_command
```

model tool listesinde GORUNMEMELI.

Bu kritik acceptance testidir.

V1 fallback modunda eski tool'lar kalabilir.

---

## Faz 7 - Browser -> Coding -> Browser E2E

Gercek hedef senaryo fixture ile otomatik test edilecek.

### Fixture

Local test app deliberately broken:

```text
UI action -> HTTP/API error
```

### E2E

1. WebBrain fixture site'i acar.
2. Browser agent aksiyonu yapar.
3. Network/console problemi tespit eder.
4. `coding_delegate` ile OMP'ye task verir.
5. OMP codebase'i kendi tools'lariyla inceler.
6. OMP edit yapar.
7. OMP ilgili test/build'i calistirir.
8. OMP browser verification ister.
9. WebBrain reload/retry yapar.
10. Browser davranisini dogrular.
11. Gerekirse failure observation'i OMP'ye follow-up yollar.
12. Ikinci edit sonrasinda browser verification basarili olur.

Test sadece mock protocol testi olmamali. En az bir gercek OMP AgentSession + fixture repo + extension/browser E2E yolu bulunmali.

Model-backed test pahali/flaky ise:

- deterministic integration harness
- kayitli fixture task
- mock model/provider path

ile CI katmani olustur; ayrica manuel/optional gercek-model E2E ekle.

---

## Faz 8 - Shadow / Parity Comparison

Ayni fixture/problem ailesini V1 ve V2 ile calistir.

Olculecekler:

```text
startup latency
coding-task time-to-first-agent-event
time-to-first-edit
total coding completion time
browser verification turnaround
total end-to-end time
number of WebBrain<->local-host messages
bytes transferred between WebBrain and local host
model-visible tool count
model input context size where measurable
CPU/RAM peak
success rate
```

V2'nin temel iddiasi sadece "daha az kod" degil, **daha hizli ve daha verimli agent architecture**dir.

Bu nedenle benchmark raporu zorunlu.

---

## Faz 9 - V2 Default, V1 Fallback

Tum acceptance testleri gectikten sonra developer/default config:

```text
workspaceBackend=omp-sdk-v2
```

olabilir.

Ancak V1 Rust fallback bir sure source tree'de kalacak.

Fallback testini gercekten calistir:

```text
switch -> rust-v1
restart/reconnect
V1 fixture passes
```

Rollback kanitlanmadan Rust silme.

---

## Faz 10 - Rust Decommission

Asagidaki kosullarin TUMU saglaninca:

- V2 provider regression green
- V2 SDK session tests green
- WebBrain V2 unit/integration green
- browser-coding-browser E2E green
- performance hedefleri kabul edilebilir
- security tests green
- rollback prova edilmis
- belirlenen soak/dev kullanimi sorun cikarmamis

Rust decommission baslat.

Silinecek/arsivlenecek adaylar:

```text
workspace-bridge/src/search.rs
workspace-bridge/src/patch.rs
workspace-bridge/src/files.rs
workspace-bridge/src/git.rs
workspace-bridge/src/command.rs
workspace-bridge/src/session.rs
... ve V1 Rust crate'in tamami
```

Ancak once dependency graph ile kullanilmadiklarini dogrula.

WebBrain'de kaldirilacak V1 code:

```text
workspace low-level tools
V1 workspace protocol client
V1-only permission mappings
V1 Rust settings/UI labels
```

Generic UI/transport kodu V2'de kullaniliyorsa koru/refactor et.

Git history korundugu icin silinen V1'i source tree'de `legacy/` klasorunde tutma; gereksiz kod yukudur.

---

# 20. PERFORMANCE HEDEFLERI

Absolute rakamlar makineye/model/provider'a bagli oldugundan yanlis bir evrensel SLA koyma.

Ancak local plumbing icin hedefler koy:

## Host transport

- localhost handshake: fark edilir gecikme yaratmamali
- WebBrain -> coding.start dispatch overhead: model latency'ye kiyasla ihmal edilebilir olmali
- progress event coalescing UI'yi flood etmemeli

## Startup

Coding Agent Plane uygulama startup'ini gereksiz yere agirlastirmamali.

Tercih:

```text
provider app starts
provider API ready
WebBrain coding module lightweight ready
AgentSession only when workspace/task actually requires it
```

Lazy session creation kullan.

## Tool surface

Testte aktif model-facing tool listesi raporlanacak.

Hedef: yalnizca gereken coding tools.

Beklenmeyen MCP/browser/computer/github/memory/task vs. varsa fail/warn.

## Handoff volume

E2E testte WebBrain<->host message ve byte sayisini olc.

V2'nin OMP internal grep/read/edit sonuclarini WebBrain'e tasimadigini kanitla.

---

# 21. RELIABILITY / ERROR HANDLING

## Host bulunamiyor

WebBrain:

```text
OMP Coding Service: disconnected
```

acik durum gostermeli.

Silent hang yok.

## SDK AgentSession create fail

Structured error:

```text
SDK_SESSION_CREATE_FAILED
```

Logda stack olabilir, browser agent context'ine full internal stack otomatik basma.

## Auth/model yok

```text
NO_AVAILABLE_MODEL
AUTH_REQUIRED
```

ayri state.

Provider API hala calisiyorsa onu dusurme.

## Task timeout

Host seviyesinde hard kill yerine once:

```text
session.abort()
```

ve kontrollu cleanup.

## Client disconnect

Policy explicit olmali:

- kisa reconnect grace period
- task devam mi eder, abort mu edilir?

Ilk surum icin guvenli tercih:

```text
short grace -> reconnect
long disconnect -> abort + dispose
```

Gercek timeout degerlerini config yap.

## Host restart

WebBrain stale task'i active sanmamali.

Connection epoch / server instance id veya esdeger mekanizma kullan.

---

# 22. OBSERVABILITY

Her coding task icin correlation id:

```text
connectionId
workspaceId
sessionId
taskId
```

Log eventleri:

```text
coding_session_created
coding_task_started
coding_task_steered
coding_task_aborted
coding_task_completed
coding_task_failed
browser_verification_requested
browser_verification_result
coding_session_disposed
```

Secrets ve full source content loglama.

Performans alanlari:

```text
sessionCreateMs
firstEventMs
firstEditMs
totalTaskMs
verificationRoundTrips
changedFileCount
checkCount
```

Debug modunda daha fazla OMP event detayi olabilir fakat production log varsayilani compact olmali.

---

# 23. TEST MATRISI

## OMP SDK App Unit Tests

- provider API unchanged
- coding WS auth
- workspace authorization
- protocol schema
- session creation
- tool restriction
- expected active tools
- AgentRegistry isolation
- abort
- dispose
- terminal agent_end detection
- progress normalization
- changed file aggregation
- error mapping

## OMP SDK Integration

- fixture repo search/edit/test
- stale edit scenario if practical
- bash test command
- LSP lazy start
- no MCP
- no ambient extensions/tools
- two sequential coding tasks same workspace
- session close/reopen

## WebBrain Unit

- V2 client connect
- coding_delegate schema
- coding progress state
- coding completed state
- coding failure state
- browser verification request
- verification result feedback
- abort
- reconnect

## WebBrain Agent Tool Exposure

In V2:

```text
coding_delegate = visible
coding_steer = visible
coding_status = visible
coding_abort = visible

workspace_search_code = hidden
workspace_read_file = hidden
workspace_apply_patch = hidden
workspace_run_command = hidden
```

## E2E

At least:

1. browser bug -> one coding fix -> browser success
2. browser bug -> coding fix -> verification fail -> second coding fix -> browser success
3. coding tests fail -> OMP self-recovers before returning
4. user aborts during coding
5. host disconnect/reconnect
6. workspace unauthorized
7. provider API works while coding task active

## Security

- non-loopback rejected
- invalid origin rejected
- bad token rejected
- expired/stale pairing rejected
- browser page cannot self-authorize workspace
- prompt injection browser content cannot grant shell/path authority
- path outside authorized workspace blocked by session/workspace creation layer
- secrets absent from logs

---

# 24. PROVIDER API REGRESSION GATE

Bu migrasyon kullanicinin mevcut provider API uygulamasini bozmamali.

Her major faz sonunda:

```text
provider health
model list (if exists)
representative completion request
streaming request (if supported)
auth/provider resolution
```

smoke testleri calistir.

Coding session agir bir test calistirirken provider API responsiveness benchmark'i da al.

Eger Agent Plane Provider Plane'i belirgin sekilde bloke ediyorsa:

1. event loop blocking kodu bul
2. CPU-heavy islemi subprocess/worker ile ayir
3. gerekirse ileride ayni repo icinde Agent Plane'i ayri process'e cikarma seam'i birak

Ancak ilk tercih ayni app/process'te moduler entegrasyondur.

---

# 25. ROLLBACK PLANI

Migrasyon rollback'i deploy sonrasi dusunulmeyecek; implementation'in parcasi olacak.

Rollback:

```text
workspaceBackend=rust-v1
```

ile calisabilmeli (Rust decommission oncesinde).

Rollback testi:

1. V2 aktif
2. sample task V2 ile basarili
3. backend flag V1'e al
4. extension reconnect/reload
5. V1 task basarili
6. tekrar V2'ye don
7. V2 task basarili

Config migration geri donusu bozuyorsa duzelt.

Rust silindikten sonraki rollback Git tabanli olur; bu nedenle Rust decommission ayri commit/PR olmalidir.

---

# 26. COMMIT STRATEJISI

Migrasyonu tek dev commit yapma.

Ornek mantiksal commitler:

```text
1. chore: capture migration baseline and backend feature flag
2. feat(omp-host): add isolated WebBrain coding service shell
3. feat(omp-host): add restricted OMP SDK coding sessions
4. feat(omp-host): add coding task protocol and normalized events
5. feat(webbrain): add OMP SDK coding backend client
6. feat(webbrain): add high-level coding handoff tools
7. test: add browser-code-browser E2E and parity benchmarks
8. chore: make omp-sdk-v2 the default backend
9. refactor: remove legacy Rust workspace backend   <-- ayri, en son
10. docs: finalize architecture and maintenance guide
```

Gercek commit sinirlari repo durumuna gore uyarla.

Rust removal'u onceki feature commitlerle karistirma.

---

# 27. UPSTREAM WEBBRAIN UPDATE-SAFE CONTRACT

V1'de oldugu gibi V2 de upstream WebBrain'e minimum invasive olmali.

WebBrain core'da ideal entegrasyon noktasi:

```text
1. high-level coding tool registration
2. coding backend adapter/client
3. permission/state UI
4. sidepanel progress
```

OMP SDK implementasyon detaylarini `agent.js` icine dagitma.

WebBrain sunu bilmemeli:

```text
createAgentSession internals
AuthStorage
ModelRegistry
OMP tool event internals
OMP edit formats
```

Bunlar local OMP SDK host'un sorumlulugudur.

WebBrain yalnızca stable coding protocol v2 bilir.

Migrasyon sonunda guncel `upstream/main` ile rehearsal yap:

- temp branch/worktree
- merge/rebase simulation
- Chrome build
- V2 tests
- conflict count

Cok sayida core conflict varsa adapter siniri yeterince iyi degildir; refactor et.

---

# 28. OMP SDK UPDATE-SAFE CONTRACT

Ayni prensip OMP icin de gecerli.

WebBrain protokolu OMP'nin ham event enum'una baglanmamali.

Tek adapter noktasi:

```text
OmpCodingSessionAdapter
```

sorumluluklari:

```text
create session
normalize events
prompt/start
steer/followup
abort
dispose
active tools self-check
result summarization
changed file/check tracking
```

OMP SDK upgrade'inde esas degisecek yer bu adapter olmali.

Dependency version pinli olsun.

Upgrade icin compatibility test:

```text
bun update / targeted SDK update
-> typecheck
-> SDK integration fixtures
-> provider regression
-> WebBrain E2E
```

---

# 29. V1'DEN KORUNACAK DEGERLI PARCALAR

Migrasyon "Rust'i sil" gorevi degildir.

Once V1'deki hangi fikirlerin V2'de gerekli oldugunu koru:

Muhtemelen korunacak:

```text
WebBrain workspace UI concepts
connection status
pairing UX
auth/origin model
browser permission semantics
test fixtures
security injection corpus
Git/upstream workflow
e2e coding-loop intent
possibly offscreen persistent WebSocket plumbing
```

Muhtemelen kaldirilacak:

```text
Rust filesystem implementation
Rust search
Rust patch
Rust revision layer
Rust git runner
Rust command runner
low-level WebBrain workspace coding tools
```

Watcher ihtiyaci ayrica degerlendirilecek.

---

# 30. NIHAI KULLANICI DENEYIMI

Kullanici WebBrain sidepanel'de sunu gorur:

```text
Workspace
--------------------------------
Connected: C:\Projects\my-app
Coding backend: OMP SDK
Model: <selected model>
Tools: 6 restricted

Task
--------------------------------
Running
Searching code...
Editing src/auth/user.ts...
Running tests...
Ready for browser verification

Browser verification
--------------------------------
Reloaded
POST /api/login -> 200
UI success state confirmed

Completed
2 files changed
18 tests passed
```

Kullanici Rust, RPC veya dusuk seviyeli filesystem bridge'i dusunmek zorunda kalmaz.

---

# 31. ORNEK TAM CAGRI AKISI

Kullanici:

> Login'e basinca 500 oluyor. Duzelt.

## A. WebBrain

```text
browser.click(Login)
network inspect
console inspect
```

Bulgu:

```text
POST /api/login -> 500
TypeError normalizeUser
```

## B. WebBrain -> OMP SDK Host

```text
coding_delegate
```

Task:

```text
Goal: Fix login 500.
Browser evidence:
- POST /api/login => 500
- console/server-related error observed: normalizeUser TypeError
Expected:
- login request must not return 5xx
- preserve existing behavior otherwise
Do the code investigation and implementation locally.
Run relevant tests.
Return changed files and a browser verification request.
```

## C. OMP SDK AgentSession

OMP kendi icinde:

```text
grep normalizeUser
read relevant files
lsp references
edit
bash relevant tests
if failure -> inspect/edit/test
```

WebBrain bu tool detaylarinin kararlarini vermez.

## D. OMP -> WebBrain

```text
completed
changedFiles = [...]
tests = passed
verification = reload + retry login + inspect request/UI
```

## E. WebBrain verification

```text
reload
click login
network inspect
UI inspect
```

Basariliysa bitir.

Basarisizsa:

```text
coding_steer(new browser observation)
```

ve ayni OMP session devam eder.

---

# 32. DEFINITION OF DONE

Migrasyon ancak asagidakilerin TAMAMI saglanirsa tamamlanmis sayilir.

## Architecture

- [ ] WebBrain browser specialist olarak calisiyor.
- [ ] OMP SDK coding specialist olarak calisiyor.
- [ ] Browser actions OMP uzerinden dolasmiyor.
- [ ] Coding file operations WebBrain/Rust uzerinden mikro-yonetilmiyor.
- [ ] Existing OMP SDK provider API davranisi korunmus.
- [ ] Provider Plane ve Agent Plane moduler ayrilmis.

## OMP SDK

- [ ] Existing app icine entegre.
- [ ] `createAgentSession` kullaniliyor.
- [ ] Restricted tool profile uygulanmis.
- [ ] Active tool self-check var.
- [ ] MCP/ambient extras beklenmedik sekilde yuklenmiyor.
- [ ] LSP lazy/configured.
- [ ] AuthStorage/ModelRegistry lifecycle dogru.
- [ ] Concurrent session varsa private AgentRegistry.
- [ ] abort/dispose dogru.

## WebBrain

- [ ] V2 coding client var.
- [ ] High-level coding handoff tool'lari var.
- [ ] V2 modunda V1 low-level workspace tools modelden gizli.
- [ ] Progress UI var.
- [ ] Browser verification loop var.
- [ ] Failed verification OMP follow-up'a donebiliyor.

## Security

- [ ] loopback only
- [ ] auth/pairing
- [ ] origin validation
- [ ] workspace authorization
- [ ] browser untrusted data separation
- [ ] secrets not logged

## Tests

- [ ] existing V1 baseline recorded
- [ ] provider regression green
- [ ] OMP SDK unit/integration green
- [ ] WebBrain unit/integration green
- [ ] browser -> coding -> browser E2E green
- [ ] verification failure -> second coding turn E2E green
- [ ] abort/reconnect tests green
- [ ] security tests green
- [ ] Chrome build green

## Performance

- [ ] V1/V2 comparison report exists
- [ ] V2 tool surface materially smaller/controlled
- [ ] internal OMP tool chatter does not cross WebBrain boundary
- [ ] no obvious regression in browser fast path
- [ ] coding handoff overhead is small relative to model execution

## Migration safety

- [ ] feature flag exists during migration
- [ ] V1 rollback tested before Rust removal
- [ ] Rust removal is separate final commit
- [ ] Git history clean
- [ ] no force push/main auto merge

## Maintenance

- [ ] WebBrain upstream rehearsal green
- [ ] OMP SDK adapter isolated
- [ ] SDK version pinned
- [ ] architecture docs updated
- [ ] final migration report written

---

# 33. FINAL DELIVERABLES

Uygulama sonunda en az:

```text
MIGRATION_BASELINE.md
MIGRATION_RESULT.md
docs/omp-sdk-coding-architecture.md
```

ve ilgili test/benchmark artifactlari bulunmali.

`MIGRATION_RESULT.md` su basliklari icermeli:

1. final architecture
2. repos/branches/remotes
3. exact files changed
4. provider regression results
5. SDK/tool profile
6. WebBrain integration
7. E2E results
8. V1 vs V2 benchmark table
9. security results
10. rollback rehearsal
11. Rust removal status
12. upstream WebBrain rehearsal
13. OMP SDK version used
14. known limitations
15. next optional improvements

---

# 34. AJAN CALISMA KURALLARI

Bu gorevi alan coding agent:

1. Once her iki repo gercegini inceler.
2. Bu dokumandaki dosya isimlerini repo gerceginin ustune zorla bindirmez.
3. Calisan V1'i ilk adimda silmez.
4. Provider API'yi yeniden yazmaz.
5. Mevcut OMP SDK app'i kullanir; gereksiz yeni OMP host repo yaratmaz.
6. OMP RPC'ye gecmez; hedef in-process SDK entegrasyonudur.
7. Rust'a yeni feature ekleyerek migrasyonu uzatmaz; Rust yalnizca temporary fallback'tir.
8. Browser tools'i OMP'ye tasimaz.
9. OMP internal tool calls'i WebBrain'e mikro-orchestrate ettirmez.
10. Testsiz refactor yapmaz.
11. Her fazda provider regression'i korur.
12. Windows 11 + Chrome/Chromium birinci hedef platformdur.
13. main'e otomatik merge yapmaz.
14. Mevcut GitHub auth'ini kullanir; secret istemez/loglamaz.
15. Mimariyi ciddi bicimde degistirmeyi gerektiren beklenmedik bir durum cikarsa `ARCHITECTURE_DECISION_REQUIRED.md` yazar, mevcut isi safe checkpoint'te birakir ve bagimsiz gorevlere devam eder.
16. Kucuk implementasyon detaylari icin kullaniciyi durmadan bekletmez; repo konvansiyonlarina gore makul karar verir.
17. Her major commit oncesi ilgili testleri calistirir.
18. Son durumda dead code, temporary debug log ve migration-only hacks temizlenir.

---

# 35. KRITIK NON-GOALS

Bu migrasyonda SU AN YAPILMAYACAK:

- OMP'yi ana browser agent yapmak
- browser DOM/network/click tool'larini OMP'ye topluca vermek
- OMP RPC process mimarisine gecmek
- Rust coding backend'i yeniden gelistirmek
- WebBrain'i IDE/editor UI'ya cevirmek
- OMP provider API'sini breaking change ile yeniden tasarlamak
- tum OMP skills/extensions/MCP/tool setini coding worker'a yuklemek
- gereksiz multi-agent/subagent orkestrasyonu eklemek
- Firefox parity icin Chrome migrasyonunu geciktirmek

Bunlar gelecekte ayri kararlar olabilir.

---

# 36. BASARI TANIMI - TEK CUMLE

Migrasyon basarili oldugunda kullanici WebBrain'de browserdaki problemi gosterecek; WebBrain problemi tarayicida hizla teshis edip mevcut yerel OMP SDK uygulamasindaki dar, stateful coding agent'a tek bir yuksek seviyeli gorev olarak devredecek; OMP codebase'i kendi native coding tools'lariyla arayip duzeltecek ve test edecek; WebBrain sonucu browserda yine kendi native extension yetenekleriyle hizla dogrulayacak; bu surecte Rust, RPC ve duplicate filesystem coding backend artik gerekmeyecek.

---

# 37. IMPLEMENTATION START COMMAND

Bu dosyayi alan ajan icin baslangic talimati:

> Read this document completely before changing code. Discover and baseline both the current WebBrain V1 workspace-bridge repository and the user's existing OMP SDK provider application. Preserve the working Rust V1 path behind a feature flag while implementing the new in-process OMP SDK Agent Plane. Do not route browser actions through OMP. Do not route coding through the old low-level WebBrain workspace tools in V2. Execute the migration phase-by-phase, validate provider regression at every major stage, build the browser->coding->browser E2E loop, benchmark V1 versus V2, rehearse rollback, then remove Rust only after all exit criteria are met. Commit in reviewable stages and push feature branches using the machine's existing GitHub authentication. Do not merge to main automatically.

---

# 38. REVISION UPDATE: BROWSER EXTENSION UI E2E & INTEGRATION HARNESS SCOPE

As part of the final execution closure, the integration harness (`test/workspace/browser-extension-e2e-harness.mjs`) is restricted to the following exact coverage:
1. Unpacked extension build output structure (`build/chrome/`).
2. Settings backend persistence and toggle between `omp-sdk-v2` and `rust-v1` rollback via workspace manager mocks.
3. Live WebSocket gateway connection and workspace open contract (`/webbrain/coding`).
4. Playwright unpacked extension loading smoke test (when browser executable is available).

**Explicit Non-Coverage**: This harness does **not** validate autonomous coding task execution (`coding.start`), progress normalization, task steering (`coding.steer`), task abort (`coding.abort`), or full multi-turn browser UI verification loops. Browser UI E2E automation remains **PARTIALLY AUTOMATED / GATED** pending headless browser extension UI runner support.

