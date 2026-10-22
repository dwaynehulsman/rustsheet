# Ontwerp Rustsheet

Rustsheet is een spreadsheet voor macOS in Rust, met de basis van Excel. Dit document legt de
belangrijkste keuzes vast: welke GUI-aanpak, hoe de engine in elkaar zit en waarom.

## Opbouw

```
crates/core   rustsheet-core: engine zonder GUI-afhankelijkheden
              addr        celadressen (A1), bereiken, limieten 1.048.576 x 16.384
              column      gepagineerde opslag per kolom
              value       waarden, foutwaarden, Excel-coercie en -vergelijking
              formula     tokenizer, Pratt-parser, AST, printen, verschuiven, evaluatie
              deps        omgekeerde afhankelijkheidsindex
              sheet       ijle celopslag, herberekening in afhankelijkheidsvolgorde
              input       getypte tekst naar celinhoud en terug
              format      getalnotaties, vet, uitlijning, Excel-datums
              clipboard   kopieren/knippen/plakken, tab-gescheiden tekst
              document    blad + ongedaan maken/opnieuw + opgeslagen-status
              io          xlsx (calamine + eigen stijllezer, rust_xlsxwriter) en CSV
crates/app    rustsheet: de macOS-app (eframe/egui op wgpu/Metal)
tools/icon    genereert het app-icoon
scripts       bundle.sh bouwt Rustsheet.app
```

De engine is volledig los van de GUI te testen (`cargo test -p rustsheet-core`).

## GUI-aanpak

Eisen: echt snel, Retina-scherp, vloeiend scrollen met trackpad, native menubalk en
Cmd-sneltoetsen, en betrouwbaar te bouwen zonder exotische afhankelijkheden.

| | gpui (Zed) | egui/eframe + wgpu | AppKit via objc2 |
|---|---|---|---|
| Rendering | Metal, zeer snel | wgpu op Metal, snel | CoreGraphics/CoreText of eigen Metal |
| Retina | ja | ja (schaalfactor van winit) | ja |
| Native menubalk | ja | nee, maar aan te vullen met `muda` | ja |
| Tekstinvoer | zelf bouwen | `TextEdit` aanwezig | `NSTextField` |
| Stabiliteit API | jong, breekt vaak, weinig documentatie | volwassen, goed gedocumenteerd | stabiel maar veel `unsafe` |
| Bouwen | Metal-shaders via Xcode-toolchain, zware dependency-boom | puur Rust plus wgpu | puur Rust plus objc2 |
| Hoeveelheid eigen code | veel (widgets, tekstvelden) | weinig | heel veel (rooster, scrollen, invoer) |

**Keuze: eframe/egui 0.37 met de wgpu-renderer**, aangevuld met:

- **muda** voor een echte macOS-menubalk (Rustsheet, Archief, Wijzig, Opmaak, Venster) met
  Cmd-sneltoetsen die macOS zelf afhandelt en in het menu toont.
- **rfd** voor de native open- en bewaarpanelen (`NSOpenPanel`, `NSSavePanel`) en
  bevestigingsdialogen.
- **arboard** voor het macOS-klembord.

Waarom niet gpui: het is de snelste optie, maar de API verandert per release, er zijn nauwelijks
kant-en-klare widgets (een tekstveld voor celbewerking moet je zelf bouwen) en de build hangt aan
de Metal-shadercompiler. Dat botst met "betrouwbaar bouwen". Waarom niet puur AppKit: een
`NSTableView` kan geen 16.384 kolommen aan, dus het rooster wordt toch zelf getekend, en dan is
egui met een eigen rooster dezelfde hoeveelheid tekenwerk met veel minder `unsafe` code.

egui tekent het rooster zelf (immediate mode), en dat past precies bij virtueel renderen: elke
frame worden alleen de zichtbare rijen en kolommen opgezocht en getekend. wgpu stuurt dat via
Metal naar de GPU, egui houdt tekstlayout in een cache, en het tekenen kost daardoor per frame
een vaste hoeveelheid werk, onafhankelijk van hoe groot het blad is.

## Engine

### Celopslag

Per kolom een vector van pagina's van 64 rijen. Een cel opzoeken is twee array-indexen, wat
telt omdat elke celverwijzing in een formule een opzoeking is. Een pagina met weinig cellen
(tot 8) is een kleine gesorteerde lijst, pas daarna een volle array; zo kost een verspreide cel
ongeveer zijn eigen grootte en kan een kwaadaardig ijl bestand geen 64-voudige geheugengroei
afdwingen. Een bereik als `A:A` (een miljoen rijen) loopt alleen langs pagina's die bestaan.
Lege cellen bestaan niet. Een cel bewaart de inhoud (letterlijke waarde of formule-AST), de
berekende waarde en de opmaak.

### Formules

- Tokenizer plus Pratt-parser met Excel-precedentie: vergelijking < `&` < `+ -` < `* /` < `^`
  < unaire min < `%`. Net als Excel is `-2^2` gelijk aan 4 en `2^3^2` aan 64.
- Een reeks operatoren op hetzelfde niveau (`A1+A2+...+A4000`) is in de AST een platte lijst,
  zodat ook de langste formule (8192 tekens, de Excel-limiet) geen diepe recursie geeft.
  Haakjes, functies en tekens zijn begrensd op 128 niveaus.
- Haakjes blijven in de AST, dus een formule wordt exact teruggeprint zoals hij gegroepeerd was.
  Functienamen en verwijzingen worden genormaliseerd naar hoofdletters.
- Argumentscheiding met `,` of `;` (Nederlandse Excel). `_xlfn.`-voorvoegsels uit xlsx worden
  weggehaald bij lezen en door rust_xlsxwriter weer toegevoegd bij schrijven.
- Functies: SUM, AVERAGE, MIN, MAX, COUNT, COUNTA, IF, IFERROR, ROUND, CONCAT, CONCATENATE,
  AND, OR, NOT, ABS, LEN, UPPER, LOWER. Onbekende functies en namen geven `#NAME?`.
- Foutwaarden `#DIV/0!`, `#REF!`, `#NAME?`, `#VALUE!`, `#NUM!`, `#N/A` en `#CIRC!` voor
  kringverwijzingen. Excel toont bij een kring een waarschuwing en 0; een zichtbare foutwaarde
  is duidelijker en propageert naar cellen die de kring lezen.
- Verwijzingen naar andere bladen geven `#REF!`: een document heeft één blad.

### Afhankelijkheden en herberekening

- `deps` houdt per formulecel de precedenten bij en daarnaast een omgekeerde index: losse
  celverwijzingen in een hashmap, bereiken in emmers van 256 rijen x 16 kolommen. Bereiken die
  meer dan 64 emmers raken (hele kolommen) staan op een aparte lijst die lineair doorlopen wordt.
- Bij een wijziging zoekt een breedte-eerst-doorloop over de omgekeerde index alle formules die
  er (transitief) van afhangen. Alleen die worden herberekend; de test
  `only_affected_cells_recalculate` controleert het precieze aantal.
- De volgorde komt uit Tarjans algoritme voor sterk samenhangende componenten, iteratief
  geschreven zodat een keten van 100.000 cellen de stack niet opblaast. Componenten komen eruit
  met precedenten eerst, wat exact de evaluatievolgorde is. Een component met meer dan één cel,
  of een cel die zichzelf leest, is een kring en krijgt `#CIRC!`.

### Ongedaan maken

Elke bewerking is een lijst van (cel, oude toestand) die `Sheet::set_states` teruggeeft;
kolombreedtes werken hetzelfde. Ongedaan maken past die lijst toe en bewaart de omgekeerde voor
opnieuw. De "gewijzigd"-status volgt een toestands-id, zodat ongedaan maken tot het opgeslagen
punt het document weer als onveranderd ziet.

### Klembord

Kopiëren zet de cellen intern vast (met formules) en de getoonde waarden als tab-gescheiden
tekst op het macOS-klembord. Bij plakken wordt gekeken of het klembord nog onze eigen tekst
bevat: zo ja, dan plakken we de interne cellen en verschuiven we relatieve verwijzingen
(`$` blijft staan, buiten het blad wordt het `#REF!`); zo nee, dan komt de tekst uit Excel of
Numbers en wordt elke waarde gelezen zoals getypte invoer (`€ 1,250.50` wordt een getal met
valutanotatie). Knippen plakt zonder verwijzingen te verschuiven en leegt daarna de bron. Past
de selectie een geheel aantal keer op het gekopieerde blok, dan wordt het blok herhaald, zoals
in Excel.

### Bestanden

- **xlsx lezen**: calamine voor waarden en formules, plus een kleine eigen lezer (zip +
  quick-xml) voor `styles.xml` en de werkbladkolommen, omdat calamine geen opmaak leest. Zo
  overleven vet, uitlijning, getalnotatie en kolombreedte een rondgang. De drie doorgangen over
  de werkblad-XML (waarden, formules, stijlen) lopen parallel. Het eerste werkblad wordt
  geladen; formules worden door onze engine opnieuw berekend.
- **xlsx schrijven**: rust_xlsxwriter, met de berekende waarde als gecachet resultaat zodat
  Excel en Quick Look meteen waarden tonen.
- **CSV**: scheidingsteken (`,`, `;` of tab) wordt herkend, UTF-8 met terugval op Windows-1252.
  Schrijven gebruikt de getoonde waarden, net als Excel. Tekst die een ander programma als
  formule zou uitvoeren (`=`, `+`, `-`, `@`, tab, CR) krijgt een `'` ervoor tegen
  CSV-formule-injectie; bij inlezen gaat die er weer af.
- Opslaan schrijft eerst naar een tijdelijk bestand en hernoemt dat dan, zodat een mislukte
  opslag nooit een half bestand achterlaat.

### Opmaak

Getal (`#,##0.00`), valuta (`€ #,##0.00`), percentage (`0.00%`), datum (`dd-mm-jjjj`), vet,
links/midden/rechts. Typen herkent `12%`, `€ 5`, `1,234.5` en `22-10-2026` en zet de bijpassende
notatie. Datums zijn Excel-serienummers in het 1900-systeem, inclusief de fictieve 29 februari
1900, zodat ze gelijk lopen met Excel.

## App

### Rooster

Het rooster is één egui-widget die zelf tekent. Elke frame worden uit de scrollpositie (in
`f64`, want een miljoen rijen van 22 punten passen niet nauwkeurig in `f32`) de zichtbare rijen
en kolommen bepaald; alleen die cellen worden opgezocht, opgemaakt en getekend. Kolomposities
volgen uit de standaardbreedte plus de afwijkende breedtes, en de kolom onder een x-positie
wordt binair gezocht. Lijnen vallen op hele fysieke pixels, zodat ze op Retina haarscherp zijn.
egui houdt de tekstlayout in een cache, dus scrollen kost vooral het opzoeken van cellen.

De schuifbalken volgen het gebruikte deel van het blad plus wat ruimte, en groeien mee als je
verder scrolt, zoals in Excel; met trackpad of pijltjes kom je altijd tot de laatste rij.

### Menubalk en sneltoetsen

`muda` bouwt de echte macOS-menubalk. Menu-items dragen de Cmd-sneltoetsen, dus macOS toont ze
en handelt ze af; de app krijgt een menugebeurtenis en wekt de UI. Commando's gaan naar het
venster dat als laatste focus had. Tijdens het bewerken van een cel gaan knippen, kopiëren,
plakken, alles selecteren en ongedaan maken naar het tekstveld (ze worden als egui-invoer
ingevoegd), anders naar het rooster. Toetsen zonder Cmd (pijltjes, Enter, Tab, Esc, F2, Delete)
handelt het rooster zelf af.

### Vensters

Elk document heeft een eigen native venster (een egui-viewport). Sluiten vraagt om te bewaren
bij wijzigingen. Het hoofdvenster kan in eframe niet los sluiten zolang er andere zijn; dan neemt
het het volgende document over en sluit dat venster. Grote bestanden worden op een
achtergrondthread gelezen, zodat vensters blijven reageren.

### Testen

`egui_kittest` draait de echte weergave zonder scherm en stuurt toets- en muisgebeurtenissen.
De tests controleren ook dat elk bedieningselement een toegankelijke naam heeft (VoiceOver).
De app kan zichzelf vastleggen zoals de GPU het frame tekende (`--screenshot`), en meet
opstarten, laden en scrollen met `--bench-scroll`.
