# Rustsheet

Een spreadsheet in Rust, geoptimaliseerd voor macOS. Een native app met de basis van Excel:
werkmappen met meerdere bladen, per blad een rooster van 1.048.576 rijen bij 16.384 kolommen,
formules met een afhankelijkheidsgraaf (ook over bladen heen), kopiëren en plakken met Excel en
Numbers, ongedaan maken, xlsx en CSV.

Het rooster wordt via wgpu op Metal getekend (Retina-scherp, vloeiend scrollen met trackpad),
met een echte macOS-menubalk en Cmd-sneltoetsen. Waarom deze aanpak: zie
[docs/ontwerp.md](docs/ontwerp.md).

## Bouwen en starten

Nodig: Rust (stable of nightly, edition 2024) en de Xcode Command Line Tools.

```bash
cargo run --release                     # starten vanuit de broncode
cargo run --release -- pad/naar/map.xlsx

scripts/bundle.sh                       # bouwt dist/Rustsheet.app
scripts/bundle.sh --open                # bouwt en opent hem
open dist/Rustsheet.app
```

`scripts/bundle.sh` bouwt de release, maakt `Rustsheet.app` met `Info.plist` (bundle-id
`nl.infodatek.rustsheet`, documenttypes voor xlsx en CSV) en icoon, en ondertekent ad hoc zodat hij lokaal zonder gedoe opent.
Met `--icon` wordt het icoon eerst opnieuw getekend (`tools/icon`, vectorieel op elke maat).

## Installeren

```bash
scripts/make-dmg.sh               # dist/Rustsheet-<versie>-arm64.dmg, met een kopie in ~/Downloads
scripts/make-dmg.sh --no-copy     # alleen in dist/
scripts/make-dmg.sh --out <map>   # kopie naar een andere map
```

De DMG bevat `Rustsheet.app` en een snelkoppeling naar Programma's: openen en de app erop
slepen. App en DMG zijn ad hoc ondertekend (`codesign -s -`); een lokaal gebouwde DMG heeft geen
quarantaine-vlag, dus de app opent zonder Gatekeeper-melding. Gedownload van elders moet je de
eerste keer rechtsklikken en Open kiezen. De build is voor Apple silicon (arm64).

Rustsheet meldt zich bij macOS als editor voor Excel-werkmappen (`.xlsx`, UTI
`org.openxmlformats.spreadsheetml.sheet`) en CSV, en als viewer voor `.xls`, `.xlsm`, `.xlsb` en
`.ods`. Daardoor staat hij in Finder onder "Open met", en een dubbelklik, het bestand op het
Dock-icoon slepen of `open -a Rustsheet map.xlsx` laadt het bestand echt, ook als Rustsheet al
draait. Een leeg, onaangeroerd venster wordt daarvoor hergebruikt, anders komt er een nieuw
venster.

## Wat er kan

**Rooster.** Virtueel getekend: alleen het zichtbare deel wordt opgezocht en getekend, dus de
grootte van het blad maakt voor het scrollen niet uit. Kolom- en rijkoppen, selecteren met muis
en pijltjes, bereik uitbreiden met Shift (pijltjes of klikken), hele kolommen of rijen via de
koppen, alles via het hoekje. Kolombreedte en rijhoogte slepen aan de rand van een kop;
dubbelklikken op een kolomrand past de breedte aan, op een rijrand zet het de standaardhoogte
terug. Tekst loopt door in lege buurcellen, getallen die niet passen worden `####`, zoals in
Excel. In het menu Weergave zet je de bovenste rij, de eerste kolom of alles boven en links van de
selectie vast; die blijven staan tijdens het scrollen.

**Rijen en kolommen.** Invoegen en verwijderen via het menu Wijzig (⇧⌘= en ⌘-) of de
rechtermuisknop op cellen en koppen; het aantal volgt de selectie. Formules op alle bladen
schuiven mee: bereiken groeien als je erin invoegt en krimpen als je eruit verwijdert,
verwijzingen naar verwijderde cellen worden `#REF!`. Kolombreedtes, rijhoogtes en filters
schuiven ook mee, en alles is ongedaan te maken.

**Bewerken.** Typen begint met bewerken, dubbelklik, Enter of F2 bewerkt de bestaande inhoud,
Esc annuleert. Enter en Tab bevestigen en gaan omlaag of naar rechts (binnen een selectie loopt
dat rond; Enter na een reeks Tabs gaat terug naar de kolom waar je begon). Tijdens het typen van een formule voegt klikken (of slepen) op een cel de verwijzing
in. De formulebalk toont en bewerkt de actieve cel; het naamvak links springt naar een adres
(`B20`, `A1:C10`). Klik je tijdens het typen van een formule op een andere tab, dan gaat het
typen verder in de formulebalk en voegt een klik op een cel `Blad2!A1` in. De statusbalk toont som, gemiddelde en aantal van de selectie.

**Werkbladen.** Een tabbalk onderaan met een tab per blad. `+` voegt een blad toe, klikken
wisselt, dubbelklikken hernoemt (Enter bevestigt, Esc annuleert), de rechtermuisknop geeft
Wijzig naam, Dupliceer, Verwijder en Nieuw blad. Wisselen kan ook met ⌃Page Up/Down of ⌥⌘←/→,
en alles staat ook in het menu Blad. Elk blad onthoudt zijn eigen selectie en scrollpositie.
Hernoemen past elke formule aan die het blad noemt, op alle bladen. Verwijderen maakt van
verwijzingen naar dat blad `#REF!`, zoals Excel. Alle bladbewerkingen zijn ongedaan te maken.

**Vullen.** Sleep het blokje rechtsonder de selectie (de vulgreep) omlaag, omhoog of opzij: twee
of meer getallen worden een reeks, een datum telt per dag door, "Week 1" wordt "Week 2", maand- en
dagnamen (Nederlands en Engels) lopen rond, formules schuiven mee en de rest wordt herhaald. Met ⌥
ingedrukt wordt alleen gekopieerd. ⌘D en ⌘R vullen omlaag en naar rechts.

**Zoeken en vervangen.** ⌘F opent de zoekbalk, Enter en ⇧Enter (of ⌘G en ⇧⌘G) gaan naar de
volgende en vorige treffer, ⌥⌘F voegt vervangen toe. Opties: hoofdletters, hele cel, zoeken in
waarden in plaats van formules. Alles vervangen is één stap om ongedaan te maken.

**Sorteren en filteren.** Menu Data (en de rechtermuisknop): sorteer oplopend of aflopend op de
kolom van de actieve cel. Staat er één cel geselecteerd, dan wordt het aaneengesloten blok
gesorteerd; een koprij wordt herkend en blijft staan. Formules gaan mee met hun rij. Filter
(⇧⌘L) zet knopjes in de koprij; per kolom kies je met vinkjes welke waarden zichtbaar blijven,
en vanuit die lijst kun je ook sorteren.

**Formules.** Getallen, tekst, `=A1+B2`, `+ - * / ^ & %`, vergelijkingen, haakjes, bereiken
`A1:B10`, hele kolommen `A:A` en rijen `3:5`, `$`-verwijzingen, verwijzingen naar andere bladen
(`Blad2!A1`, `'Mijn kosten'!B2:B9`, met aanhalingstekens bij spaties), en weggelaten argumenten
(`XLOOKUP(x,a,b,,0,-1)`). 66 functies:

- zoeken: VLOOKUP, HLOOKUP, XLOOKUP, INDEX, MATCH (exact, gesorteerd, jokertekens `*` en `?`);
- voorwaardelijk: SUMIF, SUMIFS, COUNTIF, COUNTIFS, AVERAGEIF, AVERAGEIFS, MAXIFS, MINIFS, met
  criteria als `">=10"`, `"<>"` en `"app*"`;
- rekenen: SUM, AVERAGE, MIN, MAX, MEDIAN, COUNT, COUNTA, COUNTBLANK, SUMPRODUCT, ROUND,
  ROUNDUP, ROUNDDOWN, INT, MOD, ABS, POWER, SQRT;
- logica: IF, IFS, IFERROR, IFNA, AND, OR, NOT, ISBLANK, ISNUMBER, ISTEXT, ISERROR, ISNA,
  ISLOGICAL, NA;
- tekst: TEXT (Excel-notatiecodes, Nederlandse dag- en maandnamen), CONCAT, CONCATENATE, LEFT,
  RIGHT, MID, LEN, TRIM, SUBSTITUTE, FIND, SEARCH, REPT, UPPER, LOWER, VALUE;
- datum: TODAY, NOW (herberekend bij elke wijziging), DATE, YEAR, MONTH, DAY, WEEKDAY.

Argumenten scheiden met `,` of `;`. Alleen cellen die van een wijziging afhangen worden opnieuw
berekend, ook als die op een ander blad staan. Foutwaarden `#DIV/0!`, `#REF!`, `#NAME?`, `#VALUE!`, `#NUM!` en `#CIRC!` bij een
kringverwijzing.

**Klembord.** Kopiëren, knippen en plakken via het macOS-klembord. Binnen Rustsheet blijven
formules behouden en schuiven relatieve verwijzingen mee (`$` blijft staan, buiten het blad
wordt het `#REF!`). Tab-gescheiden tekst uit Excel en Numbers wordt herkend, inclusief bedragen
als `€ 1,250.50`, percentages en datums. Een selectie die een veelvoud is van het gekopieerde
blok wordt ermee gevuld. Knippen verplaatst zonder verwijzingen te verschuiven.

**Ongedaan maken.** Cmd+Z en Cmd+Shift+Z, ook voor opmaak, plakken en kolombreedtes. Tijdens het
bewerken van een cel gelden ze voor de tekst. Delete of Backspace wist de inhoud van de selectie.

**Opmaak.** Getal, valuta, percentage en datum, vet, cursief, tekstkleur en opvulkleur (de
standaardkleuren van Excel), links/midden/rechts uitlijnen. Typen van `12%`, `€ 5`, `1,234.50`
of `22-10-2026` zet de bijpassende notatie. Alles wordt in xlsx bewaard en teruggelezen, net als
kolombreedtes, rijhoogtes, vastgezette rijen en kolommen en filters.

**Bestanden.** Openen en bewaren als `.xlsx` met alle bladen (lezen met calamine plus een eigen
stijllezer, zodat vet, uitlijning, getalnotatie en kolombreedtes behouden blijven; schrijven met
rust_xlsxwriter) en `.csv` (`,`, `;` of tab wordt herkend; CSV kent geen bladen, dus bewaren als
CSV schrijft het getoonde blad). Ook `.xls`, `.xlsm`, `.xlsb` en `.ods` kunnen worden
geopend. Een venster per document, Open recent in het Archief-menu, een vraag om te bewaren bij
sluiten, en grote bestanden laden op de achtergrond zodat het venster blijft reageren.
Bestanden kun je ook naar een venster slepen (het venster licht op; bij een onbekend formaat
volgt een melding) of op het Dock-icoon of de app in Finder laten vallen. Een leeg venster
wordt hergebruikt, anders opent het bestand in een nieuw venster, dus onbewaard werk wordt
nooit vervangen. CSV en TSV: komma, puntkomma of tab wordt herkend (`.tsv` is altijd tab),
UTF-8 met of zonder BOM, en anders Windows-1252.

**Exporteren en afdrukken.** Archief > Exporteer als PDF of CSV, en Druk af (⌘P), dat de PDF in
Voorvertoning opent om af te drukken. De PDF gebruikt de selectie als die meer dan één cel is,
anders het hele blad; pagina's worden over A4 verdeeld (liggend als dat past), vastgezette
koprijen komen op elke pagina terug, gefilterde rijen blijven weg, en opmaak, kleuren en
paginanummers staan erop.

### Sneltoetsen

| Toets | Actie |
|---|---|
| ⌘N, ⌘O, ⌘S, ⇧⌘S, ⌘W, ⌘Q | nieuw, open, bewaar, bewaar als, sluit venster, stop |
| ⌘Z, ⇧⌘Z | herstel, opnieuw |
| ⌘X, ⌘C, ⌘V, ⌘A | knip, kopieer, plak, selecteer alles |
| ⌘B, ⌘I | vet, cursief |
| ⇧⌘=, ⌘- | rijen invoegen, rijen verwijderen |
| ⌘D, ⌘R | vul omlaag, vul naar rechts |
| ⌘F, ⌘G, ⇧⌘G, ⌥⌘F | zoek, volgende, vorige, zoek en vervang |
| ⇧⌘L | filter aan/uit |
| ⌘P | druk af |
| ⇧⌘[, ⇧⌘\\, ⇧⌘] | links, midden, rechts uitlijnen |
| ⌃⇧0, 1, 3, 4, 5 | standaard, getal, datum, valuta, percentage |
| ⇧F11 | nieuw blad |
| ⌃Page Up/Down, ⌥⌘←/→ | vorig, volgend blad |
| pijltjes, ⇧pijltjes | verplaatsen, selectie uitbreiden |
| ⌘pijltjes | naar de rand van het gegevensblok, zoals in Excel |
| Enter, F2, Esc | bewerken, bevestigen, annuleren |
| Tab, ⇧Tab, ⇧Enter | bevestigen en naar rechts, links, omhoog |
| Page Up/Down, Home, ⌘Home, ⌘End | pagina, begin van de rij, A1, laatste gebruikte cel |

## Prestaties

Gemeten op een Apple M2 Max (12 kernen, 32 GB), release-build. Het testbestand heeft 100.000
rijen met 8 kolommen: 800.008 cellen, waarvan 400.000 formules (`=C2*D2`, een lopend totaal
`=F1+E2` dat een keten van 100.000 cellen vormt, `IF` en `ROUND`). Tijdens het meten draaiden
er veel andere processen (load average tussen 22 en 44), dus de wandkloktijden zijn eerder te hoog dan
te laag; daarom staat de CPU-tijd erbij.

| Wat | Wandklok | CPU |
|---|---|---|
| Opstarten tot eerste frame (leeg document) | 210 tot 250 ms | |
| xlsx met 100.000 rijen openen (lezen + alles berekenen) | 1,2 s | 1,0 s (hoofdthread) |
| Alle 400.000 formules registreren en berekenen | 413 ms | 401 ms |
| Wijziging bovenaan de keten (100.003 formules opnieuw) | 50 ms | 50 ms |
| Wijziging in de laatste rij (4 formules opnieuw) | < 0,1 ms | < 0,1 ms |
| xlsx met 100.000 rijen bewaren | 1,0 s | 0,84 s |

Scrollen door datzelfde bestand, 1.200 frames (eerst vloeiend zoals met een trackpad, daarna
grote sprongen zoals met de schuifbalk), CPU-tijd per frame:

| gemiddeld | p50 | p95 | p99 | max |
|---|---|---|---|---|
| 0,48 ms | 0,27 ms | 1,12 ms | 1,77 ms | 23,4 ms |

Een frame op een 120 Hz-scherm mag 8,3 ms duren; 99% van de frames gebruikt daar minder dan een
kwart van. De enkele uitschieter viel samen met piekbelasting van andere processen.

Zelf meten:

```bash
# testbestand maken en engine-stappen meten (wand- en CPU-tijd)
cargo run --release -p rustsheet-core --example generate -- 100000 bench-100000.xlsx

# opstarten, laden en scrollen meten in de echte app (rapport als JSON)
dist/Rustsheet.app/Contents/MacOS/rustsheet --bench-scroll --bench-frames 1200 bench-100000.xlsx
```

## Tests

```bash
cargo test                       # alles
cargo test -p rustsheet-core     # engine: parser, evaluatie, afhankelijkheden, klembord, xlsx, CSV
cargo test -p rustsheet          # interactie: toetsen en muis door de echte weergave (egui_kittest)
cargo clippy --all-targets       # zonder waarschuwingen
cargo fmt --check
```

De engine-tests dekken de formuleparser (precedentie zoals Excel, rondgang naar tekst, foute
invoer, diepte- en lengtelimieten), evaluatie en foutwaarden, kringverwijzingen, dat alleen
geraakte cellen herberekenen (met exacte aantallen), een keten van 100.000 cellen, kopiëren met
relatieve verwijzingen, tab-gescheiden tekst, ongedaan maken, lezen en schrijven van xlsx (met
meerdere bladen) en CSV, en verwijzingen tussen bladen: herberekening over bladen heen, kringen
via drie bladen, ontbrekende bladen, hernoemen (formules volgen, ook met aanhalingstekens) en
verwijderen met ongedaan maken. Verder per functiefamilie (zoeken, voorwaardelijke sommen,
tekst, datums, rekenen), notatiecodes van TEXT, rijen en kolommen invoegen en verwijderen,
vulreeksen, zoeken en vervangen, sorteren, filters, rijgeometrie en PDF-export (geldige
kruisverwijzingstabel, paginering, verborgen rijen). De interactietests sturen echte toets- en muisgebeurtenissen door de weergave: typen,
Enter/Tab/Esc/F2, Shift- en Cmd-pijltjes, wissen, kopiëren/knippen/plakken, plakken uit een andere
app, opmaak, formules aanwijzen met de muis (ook op een andere tab), dubbelklikken, kolombreedte
en rijhoogte slepen, tabs toevoegen, wisselen, hernoemen, dupliceren en verwijderen via de echte
knoppen en het contextmenu, rijen invoegen en verwijderen via de rechtermuisknop, de vulgreep,
zoeken en vervangen, kleuren uit de werkbalk, sorteren, filteren via de knopjes en vastzetten.

Voor schermafbeeldingen kan de app zichzelf vastleggen zoals de GPU het frame tekende:

```bash
cargo run --release -p rustsheet-core --example demo -- demo.xlsx
dist/Rustsheet.app/Contents/MacOS/rustsheet --screenshot-and-quit shot.png --select C4:D8 demo.xlsx
dist/Rustsheet.app/Contents/MacOS/rustsheet --screenshot-and-quit tabs.png --sheet Toelichting demo.xlsx
```

## Grenzen

Zie ook `ontbrekend.md` bij de taak. Bewust nog niet:

- grafieken, draaitabellen, voorwaardelijke opmaak en gegevensvalidatie;
- cellen samenvoegen, opmerkingen, hyperlinks, afbeeldingen, tekstterugloop in een cel;
- meerdere losse selecties (⌘-klik), tabs verslepen om bladen te ordenen;
- de volledige Excel-functiebibliotheek en matrixformules; andere functies geven `#NAME?`;
- printinstellingen (marges, schaal, kop- en voettekst kiezen): afdrukken loopt via een PDF.
