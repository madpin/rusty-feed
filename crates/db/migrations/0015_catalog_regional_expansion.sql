INSERT INTO feeds (id, url, name, site_url, description) VALUES
    (gen_random_uuid(), 'https://agenciabrasil.ebc.com.br/feed/ultimasnoticias/feed.xml', 'Agência Brasil — Últimas', 'https://agenciabrasil.ebc.com.br/', 'Brazilian public news agency coverage of public policy, society, culture, science, and national news.'),
    (gen_random_uuid(), 'https://tecnoblog.net/feed/', 'Tecnoblog', 'https://tecnoblog.net/', 'Independent Brazilian technology reporting, product analysis, digital policy, and consumer guidance.'),
    (gen_random_uuid(), 'https://theconversation.com/br/home-page.atom', 'The Conversation Brasil', 'https://theconversation.com/br', 'Academic analysis and research reporting from The Conversation Brasil.'),
    (gen_random_uuid(), 'https://www12.senado.leg.br/noticias/feed/todasnoticias/RSS', 'Senado Notícias', 'https://www12.senado.leg.br/noticias/', 'Official Brazilian Senate reporting on legislation, committees, public policy, and parliamentary debate.'),
    (gen_random_uuid(), 'https://groundup.org.za/sitenews/rss/', 'GroundUp News', 'https://groundup.org.za/', 'South African public-interest reporting on rights, health, education, housing, justice, and local communities.'),
    (gen_random_uuid(), 'https://bhekisisa.org/feed/', 'Bhekisisa', 'https://bhekisisa.org/', 'South African health journalism and science reporting on public health, evidence, and policy.'),
    (gen_random_uuid(), 'https://amabhungane.org/feed/', 'amaBhungane', 'https://amabhungane.org/', 'South African nonprofit investigative journalism on corruption, governance, and accountability.'),
    (gen_random_uuid(), 'https://rss.elconfidencial.com/espana/', 'El Confidencial España', 'https://www.elconfidencial.com/', 'Spanish reporting and analysis on national affairs, society, science, and the world.'),
    (gen_random_uuid(), 'https://feeds.weblogssl.com/xataka2', 'Xataka', 'https://www.xataka.com/', 'Spanish technology, science, digital-culture, and consumer-electronics journalism.'),
    (gen_random_uuid(), 'https://theconversation.com/es/articles.atom', 'The Conversation España', 'https://theconversation.com/es', 'Academic analysis and research reporting from The Conversation España.'),
    (gen_random_uuid(), 'https://chequeado.com/feed/', 'Chequeado', 'https://chequeado.com/', 'Argentine nonprofit fact-checking, data journalism, and public-interest reporting.'),
    (gen_random_uuid(), 'https://www.perfil.com/feed/ciencia', 'Perfil Ciencia', 'https://www.perfil.com/', 'Argentine science reporting from Perfil.'),
    (gen_random_uuid(), 'https://agenciapresentes.org/feed/', 'Agencia Presentes', 'https://agenciapresentes.org/', 'Independent Argentine journalism on human rights, gender, and LGBTQ+ communities.'),
    (gen_random_uuid(), 'https://www.heise.de/security/feed.xml', 'heise security News', 'https://www.heise.de/security/', 'German cybersecurity news, vulnerability reporting, patches, and security analysis.'),
    (gen_random_uuid(), 'https://netzpolitik.org/feed/', 'netzpolitik.org', 'https://netzpolitik.org/', 'German reporting and analysis on digital rights, technology policy, privacy, and society.'),
    (gen_random_uuid(), 'https://www.destatis.de/SiteGlobals/Functions/RSSFeed/DE/RSSNewsfeed/Aktuell.xml?nn=3624', 'Destatis Current Releases', 'https://www.destatis.de/', 'Official German statistics releases on the economy, population, labour, public finance, and society.'),
    (gen_random_uuid(), 'https://telex.hu/rss', 'Telex — Legfrissebb', 'https://telex.hu/', 'Hungarian reporting on national and international news, public affairs, science, and culture.'),
    (gen_random_uuid(), 'https://444.hu/feed/', '444', 'https://444.hu/', 'Hungarian reporting on current affairs, society, culture, and public life.'),
    (gen_random_uuid(), 'https://nki.gov.hu/figyelmeztetesek/riasztas/feed/', 'NBSZ-NKI Alerts', 'https://nki.gov.hu/', 'Official Hungarian National Cyber Security Institute alerts on vulnerabilities, malware, and active threats.'),
    (gen_random_uuid(), 'https://oko.press/feed/', 'OKO.press', 'https://oko.press/', 'Polish civic journalism, investigations, fact-checking, and reporting on rights, democracy, climate, and society.'),
    (gen_random_uuid(), 'https://naukawpolsce.pl/all/rss.xml', 'Nauka w Polsce', 'https://naukawpolsce.pl/', 'Polish science, research, higher-education, innovation, and public-interest coverage from Fundacja PAP.'),
    (gen_random_uuid(), 'https://feeds.feedburner.com/niebezpiecznik', 'Niebezpiecznik', 'https://niebezpiecznik.pl/', 'Polish cybersecurity reporting, privacy coverage, and practical security advisories.'),
    (gen_random_uuid(), 'https://meduza.io/rss/all', 'Meduza', 'https://meduza.io/', 'Independent Russian-language reporting on Russia, the region, and world affairs.'),
    (gen_random_uuid(), 'https://zona.media/rss', 'Mediazona', 'https://zona.media/', 'Independent Russian-language reporting on criminal justice, human rights, and public affairs.'),
    (gen_random_uuid(), 'https://nplus1.ru/rss', 'N + 1', 'https://nplus1.ru/', 'Russian-language science, engineering, and technology journalism.'),
    (gen_random_uuid(), 'https://www.opennet.ru/opennews/opennews_all_utf.rss', 'OpenNET', 'https://www.opennet.ru/opennews/', 'Russian-language news on free and open-source software, Linux, Unix, security, and development.'),
    (gen_random_uuid(), 'https://theins.ru/feed', 'The Insider', 'https://theins.ru/', 'Independent Russian-language investigations, fact-checking, and analysis.')
ON CONFLICT (url) DO NOTHING;

WITH catalog_seed (url, category, country, language, featured, sort_order) AS (
    VALUES
        ('https://agenciabrasil.ebc.com.br/feed/ultimasnoticias/feed.xml', 'Public Interest', 'Brazil', 'pt', false, 710),
        ('https://tecnoblog.net/feed/', 'Technology & Industry', 'Brazil', 'pt', false, 720),
        ('https://theconversation.com/br/home-page.atom', 'Research', 'Brazil', 'pt', false, 730),
        ('https://www12.senado.leg.br/noticias/feed/todasnoticias/RSS', 'Public Interest', 'Brazil', 'pt', false, 740),
        ('https://groundup.org.za/sitenews/rss/', 'Public Interest', 'South Africa', 'en', false, 750),
        ('https://bhekisisa.org/feed/', 'Science', 'South Africa', 'en', false, 760),
        ('https://amabhungane.org/feed/', 'Public Interest', 'South Africa', 'en', false, 770),
        ('https://rss.elconfidencial.com/espana/', 'World News', 'Spain', 'es', false, 780),
        ('https://feeds.weblogssl.com/xataka2', 'Technology & Industry', 'Spain', 'es', false, 790),
        ('https://theconversation.com/es/articles.atom', 'Research', 'Spain', 'es', false, 800),
        ('https://chequeado.com/feed/', 'Public Interest', 'Argentina', 'es', false, 810),
        ('https://www.perfil.com/feed/ciencia', 'Science', 'Argentina', 'es', false, 820),
        ('https://agenciapresentes.org/feed/', 'Public Interest', 'Argentina', 'es', false, 830),
        ('https://www.heise.de/security/feed.xml', 'Security & Privacy', 'Germany', 'de', false, 840),
        ('https://netzpolitik.org/feed/', 'Technology & Society', 'Germany', 'de', false, 850),
        ('https://www.destatis.de/SiteGlobals/Functions/RSSFeed/DE/RSSNewsfeed/Aktuell.xml?nn=3624', 'Business & Economy', 'Germany', 'de', false, 860),
        ('https://telex.hu/rss', 'World News', 'Hungary', 'hu', false, 870),
        ('https://444.hu/feed/', 'World News', 'Hungary', 'hu', false, 880),
        ('https://nki.gov.hu/figyelmeztetesek/riasztas/feed/', 'Security & Privacy', 'Hungary', 'hu', false, 890),
        ('https://oko.press/feed/', 'Public Interest', 'Poland', 'pl', false, 900),
        ('https://naukawpolsce.pl/all/rss.xml', 'Science', 'Poland', 'pl', false, 910),
        ('https://feeds.feedburner.com/niebezpiecznik', 'Security & Privacy', 'Poland', 'pl', false, 920),
        ('https://meduza.io/rss/all', 'World News', 'Russia', 'ru', false, 930),
        ('https://zona.media/rss', 'Public Interest', 'Russia', 'ru', false, 940),
        ('https://nplus1.ru/rss', 'Science', 'Russia', 'ru', false, 950),
        ('https://www.opennet.ru/opennews/opennews_all_utf.rss', 'Programming & Open Source', 'Russia', 'ru', false, 960),
        ('https://theins.ru/feed', 'Public Interest', 'Russia', 'ru', false, 970)
)
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order, published)
SELECT gen_random_uuid(), f.id, f.name, f.description, s.category, s.country, s.language, s.featured, s.sort_order, true
FROM catalog_seed s
JOIN feeds f ON f.url = s.url
ON CONFLICT (feed_id) DO NOTHING;
