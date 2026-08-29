INSERT INTO feeds (id, url, name, site_url, description) VALUES
    (gen_random_uuid(), 'https://news.un.org/feed/subscribe/en/news/all/rss.xml', 'UN News Top Headlines', 'https://news.un.org/', 'United Nations reporting on peace, humanitarian affairs, development, climate, and human rights.'),
    (gen_random_uuid(), 'https://www.thehindu.com/feeder/default.rss', 'The Hindu Home', 'https://www.thehindu.com/', 'National, regional, and international reporting from The Hindu.'),
    (gen_random_uuid(), 'https://feeds.elpais.com/mrss-s/pages/ep/site/elpais.com/portada', 'EL PAIS Front Page', 'https://elpais.com/', 'Spanish-language front-page reporting, analysis, and features from EL PAIS.'),
    (gen_random_uuid(), 'https://www.rnz.co.nz/rss/world.xml', 'RNZ World', 'https://www.rnz.co.nz/', 'International reporting from New Zealand public media, with Pacific relevance.'),
    (gen_random_uuid(), 'https://www.crisisgroup.org/rss.xml', 'International Crisis Group', 'https://www.crisisgroup.org/', 'Field-based conflict analysis, reports, and policy recommendations.'),
    (gen_random_uuid(), 'https://www.hrw.org/rss/news', 'Human Rights Watch News', 'https://www.hrw.org/', 'Human-rights reporting, investigations, advocacy, and country-focused analysis.'),
    (gen_random_uuid(), 'https://www.occrp.org/en/feed', 'OCCRP English', 'https://www.occrp.org/', 'Cross-border investigative reporting on organized crime and corruption.'),
    (gen_random_uuid(), 'https://www.legislation.gov.uk/new/data.feed', 'UK New Legislation', 'https://www.legislation.gov.uk/', 'Newly published United Kingdom legislation and official records.'),
    (gen_random_uuid(), 'https://www.federalreserve.gov/feeds/press_all.xml', 'Federal Reserve Press Releases', 'https://www.federalreserve.gov/', 'United States monetary policy, banking regulation, enforcement, and Board announcements.'),
    (gen_random_uuid(), 'https://www.sec.gov/news/pressreleases.rss', 'SEC Press Releases', 'https://www.sec.gov/', 'United States securities-market rules, enforcement actions, and investor-protection announcements.'),
    (gen_random_uuid(), 'https://www.ecb.europa.eu/rss/press.html', 'European Central Bank Press Releases', 'https://www.ecb.europa.eu/', 'European Central Bank policy communications, speeches, and press releases.'),
    (gen_random_uuid(), 'https://www.wto.org/library/rss/latest_news_e.xml', 'WTO Latest News', 'https://www.wto.org/', 'Trade negotiations, dispute settlement, agreements, meetings, and WTO research news.'),
    (gen_random_uuid(), 'https://blogs.worldbank.org/feed/impactevaluations/rss.xml', 'World Bank Development Impact', 'https://blogs.worldbank.org/impactevaluations/', 'Development economics, impact-evaluation methods, policy evidence, firms, and jobs.'),
    (gen_random_uuid(), 'https://www.premiumtimesng.com/category/business/feed?tztc=1', 'Premium Times Business', 'https://www.premiumtimesng.com/', 'Nigerian business, public-finance, corporate, and investigative reporting.'),
    (gen_random_uuid(), 'https://www.who.int/rss-feeds/news-english.xml', 'WHO News', 'https://www.who.int/', 'World Health Organization news on health emergencies, evidence, and global public health.'),
    (gen_random_uuid(), 'https://www.nih.gov/news-releases/feed.xml', 'NIH News Releases', 'https://www.nih.gov/', 'Biomedical research findings, health policy, clinical studies, and NIH announcements.'),
    (gen_random_uuid(), 'https://www.noaa.gov/rss.xml', 'NOAA News', 'https://www.noaa.gov/', 'Ocean, climate, weather, fisheries, environmental science, and agency news.'),
    (gen_random_uuid(), 'https://www.esa.int/rssfeed/Our_Activities/Space_Science', 'ESA Space Science', 'https://www.esa.int/', 'European Space Agency mission, astronomy, planetary-science, and research updates.'),
    (gen_random_uuid(), 'https://www.fao.org/food-safety/news/rss/en/', 'FAO Food Safety News', 'https://www.fao.org/food-safety/', 'Food-safety science, standards, risk assessment, and global food-system updates.'),
    (gen_random_uuid(), 'https://www.cisa.gov/cybersecurity-advisories/all.xml', 'CISA Cybersecurity Advisories', 'https://www.cisa.gov/', 'Actionable United States cyber alerts, advisories, malware reports, and mitigations.'),
    (gen_random_uuid(), 'https://www.ncsc.gov.uk/api/1/services/v1/news-rss-feed.xml', 'NCSC News', 'https://www.ncsc.gov.uk/', 'United Kingdom National Cyber Security Centre announcements and cyber-security news.'),
    (gen_random_uuid(), 'https://api.msrc.microsoft.com/update-guide/rss', 'Microsoft Security Update Guide', 'https://msrc.microsoft.com/update-guide/', 'Microsoft security-update summaries, revisions, CVEs, and Patch Tuesday notifications.'),
    (gen_random_uuid(), 'https://googleprojectzero.blogspot.com/feeds/posts/default', 'Google Project Zero', 'https://googleprojectzero.blogspot.com/', 'Vulnerability research, zero-day analysis, exploit root-cause analysis, and disclosure policy.'),
    (gen_random_uuid(), 'https://www.kb.cert.org/vuls/atomfeed/', 'CERT/CC Vulnerability Notes', 'https://www.kb.cert.org/', 'Coordinated vulnerability disclosures with technical details, remediation, and affected-vendor information.'),
    (gen_random_uuid(), 'https://www.smithsonianmag.com/rss/arts-culture/', 'Smithsonian Arts and Culture', 'https://www.smithsonianmag.com/', 'Journalism on art, history, literature, design, museums, and cultural heritage.'),
    (gen_random_uuid(), 'https://www.loc.gov/rss/ndnp/ndnp.xml', 'Chronicling America News', 'https://www.loc.gov/', 'Updates on historic newspapers, digitized collections, and the Chronicling America program.'),
    (gen_random_uuid(), 'https://theconversation.com/us/arts/articles.atom', 'The Conversation Arts and Culture', 'https://theconversation.com/us/', 'Editor-reviewed arts, culture, history, and humanities analysis by academic experts.'),
    (gen_random_uuid(), 'https://blog.nls.uk/feed/', 'National Library of Scotland Blog', 'https://blog.nls.uk/', 'Curator posts on collections, manuscripts, maps, digitisation, music, and Scottish history.'),
    (gen_random_uuid(), 'https://www.nga.gov/content/dam/ngaweb/press/rss/press_feed.xml', 'National Gallery of Art News', 'https://www.nga.gov/', 'Museum exhibitions, acquisitions, films, research, and press announcements.')
ON CONFLICT (url) DO NOTHING;

WITH catalog_seed (url, category, country, language, featured, sort_order) AS (
    VALUES
        ('https://news.un.org/feed/subscribe/en/news/all/rss.xml', 'World News', 'Global', 'en', true, 420),
        ('https://www.thehindu.com/feeder/default.rss', 'World News', 'India', 'en', false, 430),
        ('https://feeds.elpais.com/mrss-s/pages/ep/site/elpais.com/portada', 'World News', 'Spain', 'es', false, 440),
        ('https://www.rnz.co.nz/rss/world.xml', 'World News', 'New Zealand', 'en', false, 450),
        ('https://www.crisisgroup.org/rss.xml', 'Public Interest', 'Global', 'en', false, 460),
        ('https://www.hrw.org/rss/news', 'Public Interest', 'Global', 'en', false, 470),
        ('https://www.occrp.org/en/feed', 'Public Interest', 'Global', 'en', true, 480),
        ('https://www.legislation.gov.uk/new/data.feed', 'Public Interest', 'United Kingdom', 'en', false, 490),
        ('https://www.federalreserve.gov/feeds/press_all.xml', 'Business & Economy', 'United States', 'en', false, 500),
        ('https://www.sec.gov/news/pressreleases.rss', 'Business & Economy', 'United States', 'en', false, 510),
        ('https://www.ecb.europa.eu/rss/press.html', 'Business & Economy', 'European Union', 'en', false, 520),
        ('https://www.wto.org/library/rss/latest_news_e.xml', 'Business & Economy', 'Global', 'en', false, 530),
        ('https://blogs.worldbank.org/feed/impactevaluations/rss.xml', 'Business & Economy', 'Global', 'en', false, 540),
        ('https://www.premiumtimesng.com/category/business/feed?tztc=1', 'Business & Economy', 'Nigeria', 'en-GB', false, 550),
        ('https://www.who.int/rss-feeds/news-english.xml', 'Science', 'Global', 'en', false, 560),
        ('https://www.nih.gov/news-releases/feed.xml', 'Science', 'United States', 'en', false, 570),
        ('https://www.noaa.gov/rss.xml', 'Science', 'United States', 'en', false, 580),
        ('https://www.esa.int/rssfeed/Our_Activities/Space_Science', 'Science', 'Europe', 'en', false, 590),
        ('https://www.fao.org/food-safety/news/rss/en/', 'Science', 'Global', 'en', false, 600),
        ('https://www.cisa.gov/cybersecurity-advisories/all.xml', 'Security & Privacy', 'United States', 'en', false, 610),
        ('https://www.ncsc.gov.uk/api/1/services/v1/news-rss-feed.xml', 'Security & Privacy', 'United Kingdom', 'en', false, 620),
        ('https://api.msrc.microsoft.com/update-guide/rss', 'Security & Privacy', 'United States', 'en', false, 630),
        ('https://googleprojectzero.blogspot.com/feeds/posts/default', 'Security & Privacy', 'United States', 'en', true, 640),
        ('https://www.kb.cert.org/vuls/atomfeed/', 'Security & Privacy', 'United States', 'en', false, 650),
        ('https://www.smithsonianmag.com/rss/arts-culture/', 'Culture & Arts', 'United States', 'en', false, 660),
        ('https://www.loc.gov/rss/ndnp/ndnp.xml', 'Culture & Arts', 'United States', 'en', false, 670),
        ('https://theconversation.com/us/arts/articles.atom', 'Culture & Arts', 'United States', 'en', false, 680),
        ('https://blog.nls.uk/feed/', 'Culture & Arts', 'Scotland', 'en', false, 690),
        ('https://www.nga.gov/content/dam/ngaweb/press/rss/press_feed.xml', 'Culture & Arts', 'United States', 'en', false, 700)
)
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order, published)
SELECT gen_random_uuid(), f.id, f.name, f.description, s.category, s.country, s.language, s.featured, s.sort_order, true
FROM catalog_seed s
JOIN feeds f ON f.url = s.url
ON CONFLICT (feed_id) DO NOTHING;
