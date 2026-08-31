# Catalog feed audit — 2026-08-30

Source: <code>rss_feeds_master_list.md</code> supplied by the project owner.

## Outcome

- Candidate URLs: **173**
- Reachable RSS/Atom feeds accepted by the RustSS parser: **115**
- Already represented in the catalog after redirect/title deduplication: **12**
- Newly added by migration <code>0022_verified_catalog_feeds.sql</code>: **103**
- Published catalog total after all migrations: **200**
- Rejected for this catalog update: **58**

Validation used the production RustSS user agent, followed redirects with bounded timeouts, required a successful HTTP response with an RSS/Atom/JSON Feed root, and passed every accepted response through <code>rustss_feeds::parse_feed</code>. The arXiv <code>astro-ph</code>, <code>cs.AI</code>, and <code>physics</code> feeds plus TVN24 were structurally valid but had no entries at the moment of the audit; they were treated as live. Catalog insertion remains idempotent and does not subscribe any user.

## Existing equivalents

- TechCrunch — <code>https://techcrunch.com/feed</code>
- Ars Technica — <code>http://feeds.arstechnica.com/arstechnica/index</code>
- Wired — <code>https://www.wired.com/feed/rss</code>
- OpenAI News — <code>https://openai.com/news/rss.xml</code>
- Krebs on Security — <code>https://krebsonsecurity.com/feed/</code>
- Telex — <code>https://telex.hu/rss</code>
- 444.hu — <code>https://444.hu/rss</code>
- Nature — <code>https://www.nature.com/nature.rss</code>
- New Scientist — <code>https://www.newscientist.com/feed/home/</code>
- NASA Science — <code>https://science.nasa.gov/feed/</code>
- arXiv astro-ph — <code>https://rss.arxiv.org/rss/astro-ph</code>
- NPR Books — <code>https://feeds.npr.org/1001/rss.xml</code>

## Rejected URLs

| Feed | URL | Result |
|---|---|---|
| The Verge AI | <code>https://www.theverge.com/ai-artificial-intelligence/rss/index.xml</code> | Publisher returned HTTP 404 |
| DeepLearning.AI – The Batch | <code>https://www.deeplearning.ai/the-batch/feed/</code> | Publisher returned HTTP 404 |
| Simon Willison's Weblog | <code>https://simonwillison.net/atom.xml</code> | Publisher returned HTTP 404 |
| ByteByteGo | <code>https://blog.bytebytego.io/rss</code> | DNS name did not resolve |
| TLDR AI | <code>https://rss.beehiiv.com/feeds/tldr-ai.xml</code> | Publisher returned HTTP 404 |
| a16z Blog | <code>https://a16z.com/feed/</code> | Publisher returned HTTP 404 |
| Home Assistant Blog | <code>https://www.home-assistant.io/blog/feed.xml</code> | Publisher returned HTTP 404 |
| Zigbee2MQTT | <code>https://www.zigbee2mqtt.io/blog/feed.xml</code> | Publisher returned HTTP 404 |
| ESPHome | <code>https://esphome.io/blog/feed.xml</code> | Publisher returned HTTP 404 |
| The Verge (Smart Home) | <code>https://www.theverge.com/smart-home/rss/index.xml</code> | Publisher returned HTTP 404 |
| SmartHome Solver | <code>https://smarthomesolver.com/feed/</code> | Publisher returned HTTP 404 |
| R7 Notícias | <code>https://noticias.r7.com/feed.xml</code> | Publisher returned HTTP 404 |
| EBC Portal Brasil | <code>http://www.ebc.com.br/rss/feed.xml</code> | Publisher returned HTTP 404 |
| News24 | <code>https://www.news24.com/rss</code> | Publisher returned HTTP 404 |
| TimesLIVE | <code>https://www.timeslive.co.za/rss/</code> | Publisher returned HTTP 404 |
| TASS | <code>http://itar-tass.com/rss/v2.xml</code> | Publisher returned HTTP 403 |
| RIA Novosti | <code>https://ria.ru/export/rss2/lenta.xml</code> | Publisher returned HTTP 404 |
| PAP (Polska Agencja Prasowa) | <code>https://www.pap.pl/rss.xml</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| Gazeta Wyborcza | <code>https://wyborcza.pl/pub/rss/najnowsze.xml</code> | Publisher returned HTTP 404 |
| Magyar Nemzet | <code>https://magyarnemzet.hu/rss/</code> | Publisher returned HTTP 404 |
| Irish Examiner | <code>https://www.irishexaminer.com/rss/</code> | Publisher returned HTTP 404 |
| IrishCentral | <code>https://www.irishcentral.com/ics-rss.xml</code> | Publisher returned HTTP 406 |
| Atlas Obscura | <code>https://www.atlasobscura.com/feeds/feed</code> | Publisher returned HTTP 404 |
| The Betoota Advocate (AU) | <code>https://betootaadvocate.com/feed/</code> | Publisher returned HTTP 404 |
| Apartment Therapy | <code>https://www.apartmenttherapy.com/feed</code> | Publisher returned HTTP 404 |
| Lifehacker (Home) | <code>https://www.lifehacker.com/home/rss</code> | Publisher returned HTTP 404 |
| The Spruce (Home) | <code>https://www.thespruce.com/home-4162392/rss</code> | Publisher returned HTTP 404 |
| Instructables (Home) | <code>https://www.instructables.com/tag/type-id/Home/rss.xml</code> | Publisher returned HTTP 404 |
| Serious Eats | <code>https://www.seriouseats.com/feed</code> | Publisher returned HTTP 404 |
| Bon Appétit | <code>https://www.bonappetit.com/feed</code> | Publisher returned HTTP 404 |
| America's Test Kitchen | <code>https://www.americastestkitchen.com/rss</code> | Publisher returned HTTP 404 |
| Designboom | <code>https://www.designboom.com/designboom.xml</code> | Publisher returned HTTP 404 |
| Dwell | <code>https://www.dwell.com/rss</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| Sidebar | <code>https://sidebar.io/rss</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| Reuters Business | <code>https://feeds.reuters.com/reuters/businessNews</code> | DNS name did not resolve |
| The Motley Fool | <code>https://www.fool.com/feeds/rss.xml</code> | Publisher returned HTTP 404 |
| IMF Blog | <code>https://www.imf.org/en/Blogs/rss</code> | Publisher returned HTTP 403 |
| World Bank Blogs | <code>https://blogs.worldbank.org/feeds/all</code> | Publisher returned HTTP 404 |
| Harvard Health Blog | <code>https://www.health.harvard.edu/blog/feed/</code> | Publisher returned HTTP 404 |
| BarBend | <code>https://barbend.com/feed/</code> | Publisher returned HTTP 404 |
| T-Nation | <code>https://www.t-nation.com/feed/</code> | Publisher returned HTTP 404 |
| Bodybuilding.com Articles | <code>https://www.bodybuilding.com/rss/articles</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| Examine.com | <code>https://examine.com/feed/</code> | Publisher rate-limited the RustSS request (HTTP 429) |
| NutritionFacts.org | <code>https://nutritionfacts.org/video/feed/</code> | Publisher returned HTTP 404 |
| Psychology Today | <code>https://www.psychologytoday.com/us/rss/</code> | Publisher returned HTTP 404 |
| Publishers Weekly | <code>https://www.publishersweekly.com/rss</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| Project Gutenberg (New) | <code>https://www.gutenberg.org/browse/scores/top?format=rss</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| Standard Ebooks | <code>https://standardebooks.org/feed.xml</code> | Publisher returned HTTP 404 |
| Artsy Editorial | <code>https://www.artsy.net/editorial.rss</code> | HTTP 200, but the response was HTML or otherwise not a feed |
| DPReview News | <code>https://www.dpreview.com/news/rss</code> | Publisher returned HTTP 404 |
| Bandcamp Daily | <code>https://daily.bandcamp.com/rss</code> | Publisher returned HTTP 404 |
| MoMA Magazine | <code>https://www.moma.org/magazine/rss</code> | Publisher returned HTTP 404 |
| Lonely Planet | <code>https://www.lonelyplanet.com/rss</code> | Publisher returned HTTP 404 |
| REI Co-op Journal | <code>https://www.rei.com/blog/feed</code> | Publisher returned HTTP 403 |
| edX Blog | <code>https://www.edx.org/blog-feed</code> | Publisher returned HTTP 404 |
| Class Central | <code>https://www.classcentral.com/feed/</code> | Publisher returned HTTP 404 |
| MIT OpenCourseWare | <code>https://ocw.mit.edu/resources/rss</code> | Publisher returned HTTP 404 |
| Language Log | <code>https://languagelog.org/feed/</code> | Connection timed out |

These URLs were deliberately not seeded. A future audit may add them if the publisher restores a machine-readable feed that RustSS can fetch without credentials or bypasses.
