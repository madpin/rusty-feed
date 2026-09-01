-- Curated feeds verified live on 2026-08-30 with the Rusty Feed user agent and
-- parsed through rusty_feed_feeds::parse_feed. Redirect-equivalent and title-
-- equivalent entries already in the catalog are intentionally omitted.
WITH seed (url, title, description, category, country, language, sort_order) AS (
    VALUES
        ('https://www.theverge.com/rss/index.xml', 'The Verge', 'Articles and updates from The Verge covering General Tech & Internet.', 'Technology & Industry', 'Global', 'en', 980),
        ('https://www.engadget.com/rss.xml', 'Engadget', 'Articles and updates from Engadget covering General Tech & Internet.', 'Technology & Industry', 'Global', 'en', 990),
        ('https://hnrss.org/frontpage', 'Hacker News (Front Page)', 'Articles and updates from Hacker News (Front Page) covering General Tech & Internet.', 'Technology & Industry', 'Global', 'en', 1000),
        ('http://rss.slashdot.org/Slashdot/slashdotMain', 'Slashdot', 'Articles and updates from Slashdot covering General Tech & Internet.', 'Technology & Industry', 'Global', 'en', 1010),
        ('https://www.technologyreview.com/feed', 'MIT Technology Review', 'Articles and updates from MIT Technology Review covering General Tech & Internet.', 'Technology & Industry', 'Global', 'en', 1020),
        ('https://www.techradar.com/feeds.xml', 'TechRadar', 'Articles and updates from TechRadar covering General Tech & Internet.', 'Technology & Industry', 'Global', 'en', 1030),
        ('https://huggingface.co/blog/feed.xml', 'Hugging Face Blog', 'Articles and updates from Hugging Face Blog covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1040),
        ('https://blog.google/technology/ai/rss/', 'Google AI Blog', 'Articles and updates from Google AI Blog covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1050),
        ('https://www.marktechpost.com/feed/', 'MarkTechPost', 'Articles and updates from MarkTechPost covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1060),
        ('https://rss.arxiv.org/rss/cs.AI', 'arXiv cs.AI', 'Articles and updates from arXiv cs.AI covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1070),
        ('https://techcrunch.com/category/artificial-intelligence/feed/', 'TechCrunch AI', 'Articles and updates from TechCrunch AI covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1080),
        ('https://venturebeat.com/category/ai/feed/', 'VentureBeat AI', 'Articles and updates from VentureBeat AI covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1090),
        ('https://www.technologyreview.com/topic/artificial-intelligence/feed/', 'MIT Tech Review (AI)', 'Articles and updates from MIT Tech Review (AI) covering AI & Machine Learning.', 'AI & Research', 'Global', 'en', 1100),
        ('https://rss.beehiiv.com/feeds/2R3C6Bt5wj.xml', 'Ben''s Bites', 'Articles and updates from Ben''s Bites covering Developer & Internet Engineering.', 'Programming & Open Source', 'Global', 'en', 1110),
        ('https://stratechery.com/feed/', 'Stratechery', 'Articles and updates from Stratechery covering Developer & Internet Engineering.', 'Programming & Open Source', 'Global', 'en', 1120),
        ('https://github.blog/feed/', 'GitHub Blog', 'Articles and updates from GitHub Blog covering Developer & Internet Engineering.', 'Programming & Open Source', 'Global', 'en', 1130),
        ('https://devops.com/feed/', 'DevOps.com', 'Articles and updates from DevOps.com covering Developer & Internet Engineering.', 'Programming & Open Source', 'Global', 'en', 1140),
        ('https://lwn.net/headlines/newrss', 'LWN.net', 'Articles and updates from LWN.net covering Developer & Internet Engineering.', 'Programming & Open Source', 'Global', 'en', 1150),
        ('https://hackaday.com/feed/', 'Hackaday', 'Articles and updates from Hackaday covering IoT, Smart Home & Embedded.', 'Technology & Industry', 'Global', 'en', 1160),
        ('https://blog.adafruit.com/feed/', 'Adafruit Blog', 'Articles and updates from Adafruit Blog covering IoT, Smart Home & Embedded.', 'Technology & Industry', 'Global', 'en', 1170),
        ('https://www.schneier.com/feed/atom/', 'Schneier on Security', 'Articles and updates from Schneier on Security covering Security & Privacy.', 'Security & Privacy', 'Global', 'en', 1180),
        ('https://feeds.feedburner.com/TheHackersNews', 'The Hacker News', 'Articles and updates from The Hacker News covering Security & Privacy.', 'Security & Privacy', 'Global', 'en', 1190),
        ('https://feeds.folha.uol.com.br/emcimadahora/rss091.xml', 'Folha de S.Paulo (Em Cima da Hora)', 'Current Brazil reporting and updates from Folha de S.Paulo (Em Cima da Hora).', 'World News', 'Brazil', 'pt', 1200),
        ('http://rss.home.uol.com.br/index.xml', 'UOL Notícias', 'Current Brazil reporting and updates from UOL Notícias.', 'World News', 'Brazil', 'pt', 1210),
        ('https://riotimesonline.com/feed/', 'The Rio Times (English)', 'Current Brazil reporting and updates from The Rio Times (English).', 'World News', 'Brazil', 'en', 1220),
        ('https://www.brasildefato.com.br/feed', 'Brasil de Fato', 'Current Brazil reporting and updates from Brasil de Fato.', 'World News', 'Brazil', 'pt', 1230),
        ('https://jornaldebrasilia.com.br/feed/', 'Jornal de Brasília', 'Current Brazil reporting and updates from Jornal de Brasília.', 'World News', 'Brazil', 'pt', 1240),
        ('https://www.agazeta.com.br/rss', 'A Gazeta (ES)', 'Current Brazil reporting and updates from A Gazeta (ES).', 'World News', 'Brazil', 'pt', 1250),
        ('https://rss.iol.io/iol/news', 'IOL News', 'Current South Africa reporting and updates from IOL News.', 'World News', 'South Africa', 'en', 1260),
        ('https://www.dailymaverick.co.za/dmrss', 'Daily Maverick', 'Current South Africa reporting and updates from Daily Maverick.', 'World News', 'South Africa', 'en', 1270),
        ('https://www.thesouthafrican.com/feed', 'The South African', 'Current South Africa reporting and updates from The South African.', 'World News', 'South Africa', 'en', 1280),
        ('https://www.moneyweb.co.za/feed', 'Moneyweb (business)', 'Current South Africa reporting and updates from Moneyweb (business).', 'Business & Economy', 'South Africa', 'en', 1290),
        ('http://russian.rt.com/rss/', 'RT (Russian)', 'Current Russia reporting and updates from RT (Russian).', 'World News', 'Russia', 'ru', 1300),
        ('https://aif.ru/rss/news.php', 'AIF.ru', 'Current Russia reporting and updates from AIF.ru.', 'World News', 'Russia', 'ru', 1310),
        ('https://www.interfax.ru/rss/', 'Interfax', 'Current Russia reporting and updates from Interfax.', 'World News', 'Russia', 'ru', 1320),
        ('https://www.newsweek.pl/rss.xml', 'Newsweek Polska', 'Current Poland reporting and updates from Newsweek Polska.', 'World News', 'Poland', 'pl', 1330),
        ('http://rss.dziennik.pl/Dziennik-PL/', 'Dziennik.pl', 'Current Poland reporting and updates from Dziennik.pl.', 'World News', 'Poland', 'pl', 1340),
        ('https://www.wirtualnemedia.pl/rss/wirtualnemedia_rss.xml', 'Wirtualnemedia.pl', 'Current Poland reporting and updates from Wirtualnemedia.pl.', 'World News', 'Poland', 'pl', 1350),
        ('http://rss.gazetaprawna.pl/GazetaPrawna', 'GazetaPrawna.pl', 'Current Poland reporting and updates from GazetaPrawna.pl.', 'World News', 'Poland', 'pl', 1360),
        ('https://www.rp.pl/rss/1019', 'Rzeczpospolita', 'Current Poland reporting and updates from Rzeczpospolita.', 'World News', 'Poland', 'pl', 1370),
        ('https://www.rmf24.pl/feed', 'RMF24', 'Current Poland reporting and updates from RMF24.', 'World News', 'Poland', 'pl', 1380),
        ('https://tvn24.pl/najwazniejsze.xml', 'TVN24', 'Current Poland reporting and updates from TVN24.', 'World News', 'Poland', 'pl', 1390),
        ('https://nepszava.hu/rss', 'Népszava', 'Current Hungary reporting and updates from Népszava.', 'World News', 'Hungary', 'hu', 1400),
        ('https://hvg.hu/rss', 'HVG', 'Current Hungary reporting and updates from HVG.', 'World News', 'Hungary', 'hu', 1410),
        ('https://dailynewshungary.com/feed/', 'Daily News Hungary (English)', 'Current Hungary reporting and updates from Daily News Hungary (English).', 'World News', 'Hungary', 'en', 1420),
        ('https://www.thejournal.ie/feed/', 'TheJournal.ie', 'Current Ireland reporting and updates from TheJournal.ie.', 'World News', 'Ireland', 'en', 1430),
        ('https://feeds.breakingnews.ie/bntopstories', 'BreakingNews.ie', 'Current Ireland reporting and updates from BreakingNews.ie.', 'World News', 'Ireland', 'en', 1440),
        ('https://www.the42.ie/feed/', 'The42 (sport)', 'Current Ireland reporting and updates from The42 (sport).', 'Sports', 'Ireland', 'en', 1450),
        ('https://www.irishmirror.ie/?service=rss', 'Irish Mirror', 'Current Ireland reporting and updates from Irish Mirror.', 'World News', 'Ireland', 'en', 1460),
        ('https://phys.org/rss-feed/', 'Phys.org', 'Articles and updates from Phys.org covering Science.', 'Science', 'Global', 'en', 1470),
        ('https://www.sciencedaily.com/rss/all.xml', 'ScienceDaily', 'Articles and updates from ScienceDaily covering Science.', 'Science', 'Global', 'en', 1480),
        ('https://www.sciencenews.org/feed/', 'Science News', 'Articles and updates from Science News covering Science.', 'Science', 'Global', 'en', 1490),
        ('https://www.wired.com/feed/category/science/latest/rss', 'Wired Science', 'Articles and updates from Wired Science covering Science.', 'Science', 'Global', 'en', 1500),
        ('https://www.nasa.gov/rss/dyn/breaking_news.rss', 'NASA Breaking News', 'Articles and updates from NASA Breaking News covering Science.', 'Science', 'Global', 'en', 1510),
        ('https://rss.arxiv.org/rss/physics', 'arXiv physics', 'Articles and updates from arXiv physics covering Science.', 'Research', 'Global', 'en', 1520),
        ('https://aeon.co/feed.rss', 'Aeon', 'Articles and updates from Aeon covering Curiosity / Interesting Stuff.', 'Culture & Arts', 'Global', 'en', 1530),
        ('https://waitbutwhy.com/feed', 'Wait But Why', 'Articles and updates from Wait But Why covering Curiosity / Interesting Stuff.', 'Culture & Arts', 'Global', 'en', 1540),
        ('https://fs.blog/feed/', 'Farnam Street', 'Articles and updates from Farnam Street covering Curiosity / Interesting Stuff.', 'Culture & Arts', 'Global', 'en', 1550),
        ('https://www.themarginalian.org/feed/', 'The Marginalian', 'Articles and updates from The Marginalian covering Curiosity / Interesting Stuff.', 'Culture & Arts', 'Global', 'en', 1560),
        ('https://www.openculture.com/feed/', 'Open Culture', 'Articles and updates from Open Culture covering Curiosity / Interesting Stuff.', 'Culture & Arts', 'Global', 'en', 1570),
        ('https://kottke.org/index.xml', 'Kottke', 'Articles and updates from Kottke covering Curiosity / Interesting Stuff.', 'Culture & Arts', 'Global', 'en', 1580),
        ('https://theonion.com/feed/', 'The Onion', 'Articles and updates from The Onion covering Comedy & Humor.', 'Culture & Arts', 'Global', 'en', 1590),
        ('https://reductress.com/feed/', 'Reductress', 'Articles and updates from Reductress covering Comedy & Humor.', 'Culture & Arts', 'Global', 'en', 1600),
        ('https://www.thedailymash.co.uk/rss', 'The Daily Mash (UK)', 'Articles and updates from The Daily Mash (UK) covering Comedy & Humor.', 'Culture & Arts', 'Global', 'en', 1610),
        ('https://xkcd.com/atom.xml', 'xkcd', 'Articles and updates from xkcd covering Comedy & Humor.', 'Culture & Arts', 'Global', 'en', 1620),
        ('https://www.mcsweeneys.net/rss', 'McSweeney''s Internet Tendency', 'Articles and updates from McSweeney''s Internet Tendency covering Comedy & Humor.', 'Culture & Arts', 'Global', 'en', 1630),
        ('https://thehardtimes.net/feed/', 'The Hard Times', 'Articles and updates from The Hard Times covering Comedy & Humor.', 'Culture & Arts', 'Global', 'en', 1640),
        ('https://www.gardenista.com/feed/', 'Gardenista', 'Articles and updates from Gardenista covering Home, DIY, Gardening, Cooking.', 'Lifestyle & Home', 'Global', 'en', 1650),
        ('https://www.dezeen.com/feed/', 'Dezeen', 'Articles and updates from Dezeen covering Design & Architecture.', 'Culture & Arts', 'Global', 'en', 1660),
        ('https://www.archdaily.com/feed', 'ArchDaily', 'Articles and updates from ArchDaily covering Design & Architecture.', 'Culture & Arts', 'Global', 'en', 1670),
        ('https://www.core77.com/feed', 'Core77', 'Articles and updates from Core77 covering Design & Architecture.', 'Culture & Arts', 'Global', 'en', 1680),
        ('https://www.remodelista.com/feed/', 'Remodelista', 'Articles and updates from Remodelista covering Design & Architecture.', 'Culture & Arts', 'Global', 'en', 1690),
        ('https://uxdesign.cc/feed', 'UX Collective', 'Articles and updates from UX Collective covering Design & Architecture.', 'Culture & Arts', 'Global', 'en', 1700),
        ('https://www.smashingmagazine.com/feed/', 'Smashing Magazine', 'Articles and updates from Smashing Magazine covering Design & Architecture.', 'Culture & Arts', 'Global', 'en', 1710),
        ('https://search.cnbc.com/rs/search/combinedcms/view.xml?partnerId=wrss01&id=100003114', 'CNBC Top News', 'Articles and updates from CNBC Top News covering Markets & Business News.', 'Business & Economy', 'Global', 'en', 1720),
        ('https://feeds.marketwatch.com/marketwatch/topstories/', 'MarketWatch Top Stories', 'Articles and updates from MarketWatch Top Stories covering Markets & Business News.', 'Business & Economy', 'Global', 'en', 1730),
        ('https://www.ft.com/global-trade?format=rss', 'Financial Times', 'Articles and updates from Financial Times covering Markets & Business News.', 'Business & Economy', 'Global', 'en', 1740),
        ('https://www.economist.com/finance-and-economics/rss.xml', 'The Economist (Finance)', 'Articles and updates from The Economist (Finance) covering Markets & Business News.', 'Business & Economy', 'Global', 'en', 1750),
        ('https://www.mrmoneymustache.com/feed/', 'Mr. Money Mustache', 'Articles and updates from Mr. Money Mustache covering Personal Finance & Investing.', 'Business & Economy', 'Global', 'en', 1760),
        ('https://awealthofcommonsense.com/feed/', 'A Wealth of Common Sense', 'Articles and updates from A Wealth of Common Sense covering Personal Finance & Investing.', 'Business & Economy', 'Global', 'en', 1770),
        ('https://www.stackingbenjamins.com/feed/', 'The Stacking Benjamins Show', 'Articles and updates from The Stacking Benjamins Show covering Personal Finance & Investing.', 'Business & Economy', 'Global', 'en', 1780),
        ('https://www.project-syndicate.org/rss', 'Project Syndicate', 'Articles and updates from Project Syndicate covering Macro / Policy.', 'Business & Economy', 'Global', 'en', 1790),
        ('https://www.precisionnutrition.com/feed', 'Precision Nutrition', 'Articles and updates from Precision Nutrition covering Health, Fitness & Nutrition.', 'Health & Wellness', 'Global', 'en', 1800),
        ('https://rss.nytimes.com/services/xml/rss/nyt/Books.xml', 'NYT Books', 'Articles and updates from NYT Books covering Books, Literature & Culture.', 'Culture & Arts', 'Global', 'en', 1810),
        ('https://www.theguardian.com/books/rss', 'The Guardian Books', 'Articles and updates from The Guardian Books covering Books, Literature & Culture.', 'Culture & Arts', 'Global', 'en', 1820),
        ('https://lithub.com/feed/', 'Literary Hub', 'Articles and updates from Literary Hub covering Books, Literature & Culture.', 'Culture & Arts', 'Global', 'en', 1830),
        ('https://www.theparisreview.org/blog/feed/', 'The Paris Review Daily', 'Articles and updates from The Paris Review Daily covering Books, Literature & Culture.', 'Culture & Arts', 'Global', 'en', 1840),
        ('https://hyperallergic.com/feed/', 'Hyperallergic', 'Articles and updates from Hyperallergic covering Art, Photography & Music.', 'Culture & Arts', 'Global', 'en', 1850),
        ('https://www.thisiscolossal.com/feed/', 'Colossal', 'Articles and updates from Colossal covering Art, Photography & Music.', 'Culture & Arts', 'Global', 'en', 1860),
        ('https://petapixel.com/feed/', 'PetaPixel', 'Articles and updates from PetaPixel covering Art, Photography & Music.', 'Culture & Arts', 'Global', 'en', 1870),
        ('https://www.fstoppers.com/feed', 'Fstoppers', 'Articles and updates from Fstoppers covering Art, Photography & Music.', 'Culture & Arts', 'Global', 'en', 1880),
        ('https://pitchfork.com/rss/news/', 'Pitchfork News', 'Articles and updates from Pitchfork News covering Art, Photography & Music.', 'Culture & Arts', 'Global', 'en', 1890),
        ('https://www.thefader.com/feed', 'The FADER', 'Articles and updates from The FADER covering Art, Photography & Music.', 'Culture & Arts', 'Global', 'en', 1900),
        ('https://skift.com/feed/', 'Skift (travel industry)', 'Articles and updates from Skift (travel industry) covering Travel, Outdoors & Adventure.', 'Travel & Outdoors', 'Global', 'en', 1910),
        ('https://thepointsguy.com/feed/', 'The Points Guy', 'Articles and updates from The Points Guy covering Travel, Outdoors & Adventure.', 'Travel & Outdoors', 'Global', 'en', 1920),
        ('https://www.outsideonline.com/feed/', 'Outside Online', 'Articles and updates from Outside Online covering Travel, Outdoors & Adventure.', 'Travel & Outdoors', 'Global', 'en', 1930),
        ('https://adventure-journal.com/feed/', 'Adventure Journal', 'Articles and updates from Adventure Journal covering Travel, Outdoors & Adventure.', 'Travel & Outdoors', 'Global', 'en', 1940),
        ('https://simpleflying.com/feed/', 'Simple Flying', 'Articles and updates from Simple Flying covering Travel, Outdoors & Adventure.', 'Travel & Outdoors', 'Global', 'en', 1950),
        ('https://onemileatatime.com/feed/', 'One Mile at a Time', 'Articles and updates from One Mile at a Time covering Travel, Outdoors & Adventure.', 'Travel & Outdoors', 'Global', 'en', 1960),
        ('https://blog.coursera.org/feed/', 'Coursera Blog', 'Articles and updates from Coursera Blog covering Education & Learning.', 'Education & Learning', 'Global', 'en', 1970),
        ('https://www.freecodecamp.org/news/rss/', 'FreeCodeCamp News', 'Articles and updates from FreeCodeCamp News covering Education & Learning.', 'Education & Learning', 'Global', 'en', 1980),
        ('https://blog.khanacademy.org/feed/', 'Khan Academy Blog', 'Articles and updates from Khan Academy Blog covering Education & Learning.', 'Education & Learning', 'Global', 'en', 1990),
        ('https://www.fluentin3months.com/feed/', 'Fluent in 3 Months', 'Articles and updates from Fluent in 3 Months covering Education & Learning.', 'Education & Learning', 'Global', 'en', 2000)
), inserted_feeds AS (
    INSERT INTO feeds (id, url, name, site_url, description)
    SELECT gen_random_uuid(), seed.url, seed.title, NULL, seed.description FROM seed
    ON CONFLICT (url) DO NOTHING
    RETURNING id, url
), resolved_feeds AS (
    SELECT id, url FROM inserted_feeds
    UNION ALL
    SELECT feed.id, feed.url FROM feeds feed
    JOIN seed ON seed.url = feed.url
    WHERE NOT EXISTS (SELECT 1 FROM inserted_feeds inserted WHERE inserted.url = feed.url)
)
INSERT INTO feed_catalog_entries
    (id, feed_id, title, description, category, country, language, featured, sort_order, published)
SELECT gen_random_uuid(), resolved.id, seed.title, seed.description, seed.category,
       seed.country, seed.language, false, seed.sort_order, true
FROM seed
JOIN resolved_feeds resolved ON resolved.url = seed.url
ON CONFLICT (feed_id) DO NOTHING;

