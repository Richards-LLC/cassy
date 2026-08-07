---
name: cas-seo-expert
description: SEO audit, optimization, and implementation for web apps. Use when auditing pages for SEO, implementing meta tags, structured data, sitemaps, robots.txt, Open Graph, schema.org markup, Core Web Vitals, or optimizing for search engines and AI search.
user-invocable: false
---

# seo-expert

# SEO Expert

## Audit Framework

When auditing a page or site, evaluate these categories in order of impact:

### 1. Crawlability & Indexing
- **robots.txt** — verify it exists, allows important paths, blocks admin/auth pages
- **Sitemap** — XML sitemap exists, auto-generated, includes all public pages, submitted to Search Console
- **Canonical URLs** — every page has `<link rel="canonical">`, no duplicate content
- **Noindex directives** — auth-required pages should be noindex, public pages should be indexable
- **HTTP status codes** — no soft 404s, proper 301 redirects for moved pages
- **Crawl depth** — important pages reachable within 3 clicks from homepage

### 2. Meta Tags (Per Page)
Every public page must have:
```html
<title>Primary Keyword — Brand Name</title>  <!-- 50-60 chars -->
<meta name="description" content="Compelling description with keywords"> <!-- 150-160 chars -->
<link rel="canonical" href="https://example.com/page">
```

Open Graph (social sharing):
```html
<meta property="og:title" content="Page Title">
<meta property="og:description" content="Description">
<meta property="og:image" content="https://example.com/og-image.jpg"> <!-- 1200x630px -->
<meta property="og:url" content="https://example.com/page">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Brand Name">
```

Twitter Card:
```html
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="Page Title">
<meta name="twitter:description" content="Description">
<meta name="twitter:image" content="https://example.com/twitter-image.jpg">
```

### 3. Heading Hierarchy
- Exactly ONE `<h1>` per page containing the primary keyword
- `<h2>` for major sections, `<h3>` for subsections
- No skipped levels (h1 → h3 without h2)
- Headings should be descriptive, not generic ("Our Services" → "Ayurvedic Health Services")

### 4. Structured Data (Schema.org JSON-LD)

**Organization** (site-wide, in layout):
```json
{
  "@context": "https://schema.org",
  "@type": "Organization",
  "name": "Brand Name",
  "url": "https://example.com",
  "logo": "https://example.com/logo.png",
  "sameAs": ["https://twitter.com/brand", "https://facebook.com/brand"],
  "contactPoint": {
    "@type": "ContactPoint",
    "telephone": "+1-xxx-xxx-xxxx",
    "contactType": "customer service"
  }
}
```

**WebSite** (homepage):
```json
{
  "@context": "https://schema.org",
  "@type": "WebSite",
  "name": "Brand Name",
  "url": "https://example.com",
  "potentialAction": {
    "@type": "SearchAction",
    "target": "https://example.com/search?q={search_term_string}",
    "query-input": "required name=search_term_string"
  }
}
```

**HealthBusiness / MedicalBusiness** (for health platforms):
```json
{
  "@context": "https://schema.org",
  "@type": "MedicalBusiness",
  "name": "Practice Name",
  "medicalSpecialty": "Ayurvedic Medicine",
  "availableService": {
    "@type": "MedicalTherapy",
    "name": "Health Assessment"
  }
}
```

**Person / Physician** (provider profiles):
```json
{
  "@context": "https://schema.org",
  "@type": "Person",
  "name": "Dr. Provider Name",
  "jobTitle": "Ayurvedic Practitioner",
  "worksFor": { "@type": "Organization", "name": "Brand" },
  "knowsAbout": ["Ayurveda", "Holistic Health"]
}
```

**FAQPage** (FAQ sections):
```json
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [{
    "@type": "Question",
    "name": "Question text?",
    "acceptedAnswer": {
      "@type": "Answer",
      "text": "Answer text."
    }
  }]
}
```

**BlogPosting** (blog/article pages):
```json
{
  "@context": "https://schema.org",
  "@type": "BlogPosting",
  "headline": "Article Title",
  "author": { "@type": "Person", "name": "Author" },
  "datePublished": "2026-01-15",
  "dateModified": "2026-03-20",
  "image": "https://example.com/article-image.jpg"
}
```

Validate with: https://search.google.com/test/rich-results

### 5. Core Web Vitals
- **LCP (Largest Contentful Paint)** < 2.5s — optimize hero images, preload critical assets, use SSR/prerender
- **INP (Interaction to Next Paint)** < 200ms — minimize JS bundle, defer non-critical scripts
- **CLS (Cumulative Layout Shift)** < 0.1 — set explicit width/height on images/video, avoid layout shifts from dynamic content

Key patterns:
- Preload hero image: `<link rel="preload" as="image" href="hero.webp">`
- Lazy load below-fold images: `loading="lazy"`
- Use `font-display: swap` for web fonts
- Serve images in WebP/AVIF with proper srcset

### 6. Image Optimization
- Every `<img>` must have descriptive `alt` text (not "image1.jpg")
- Use WebP/AVIF formats with fallbacks
- Serve responsive images with `srcset` and `sizes`
- Lazy load images below the fold
- Set explicit `width` and `height` to prevent CLS
- Compress images — target < 100KB for thumbnails, < 300KB for heroes

### 7. Internal Linking & Navigation
- Clear nav structure with descriptive anchor text
- Breadcrumbs on all subpages (with BreadcrumbList schema)
- Related content links within body text
- Footer links to key pages
- No orphan pages (every page linked from at least one other)

### 8. Mobile SEO
- `<meta name="viewport" content="width=device-width, initial-scale=1">`
- Touch targets minimum 48x48px
- No horizontal scroll
- Font size minimum 16px for body text
- Test with Google Mobile-Friendly Test

### 9. AI Search Optimization (GEO)
Modern SEO must account for AI-powered search (Google AI Overviews, Bing Copilot, Perplexity):
- Use clear, factual language that AI can extract
- Structure content as Q&A where possible
- Include authoritative citations and data points
- Use bullet points and numbered lists for scannability
- Add author bylines with credentials (E-E-A-T signals)
- Consider `robots.txt` rules for AI crawlers:
```
User-agent: GPTBot
Allow: /blog
Disallow: /private

User-agent: ChatGPT-User
Allow: /
```

### 10. E-E-A-T (Experience, Expertise, Authoritativeness, Trustworthiness)
Critical for health/wellness sites (YMYL — Your Money Your Life):
- Author bios with credentials on content pages
- About page with team credentials
- Trust signals: certifications, partnerships, testimonials
- Contact information easily accessible
- Privacy policy and terms of service
- HTTPS (non-negotiable)
- No misleading claims or unsubstantiated health advice

## Nuxt-Specific Implementation

### Using @nuxtjs/seo module
```ts
// nuxt.config.ts
export default defineNuxtConfig({
  modules: ['@nuxtjs/seo'],
  site: {
    url: 'https://example.com',
    name: 'Site Name',
    description: 'Site description',
    defaultLocale: 'en',
  },
  seo: {
    meta: {
      description: 'Default description',
      ogImage: '/og-default.jpg',
      twitterCard: 'summary_large_image',
    }
  }
})
```

### Per-page SEO with useSeoMeta
```vue
<script setup>
useSeoMeta({
  title: 'Page Title — Brand',
  description: 'Page description with keywords',
  ogTitle: 'Page Title',
  ogDescription: 'Page description',
  ogImage: '/images/page-og.jpg',
  ogUrl: 'https://example.com/page',
  twitterCard: 'summary_large_image',
})
</script>
```

### Schema.org with useSchemaOrg
```vue
<script setup>
useSchemaOrg([
  defineWebPage({ name: 'Page Title' }),
  defineOrganization({
    name: 'Brand',
    logo: '/logo.png',
    sameAs: ['https://twitter.com/brand']
  })
])
</script>
```

### Robots configuration
```ts
// nuxt.config.ts
export default defineNuxtConfig({
  robots: {
    disallow: ['/account', '/management', '/signin', '/signup'],
    groups: [
      { userAgent: ['GPTBot', 'ChatGPT-User'], allow: ['/blog', '/providers'], disallow: ['/account'] }
    ]
  }
})
```

## Audit Output Format

When performing an audit, structure findings as:

| Priority | Category | Issue | Page | Fix |
|----------|----------|-------|------|-----|
| P0 | Indexing | No sitemap.xml | Site-wide | Add @nuxtjs/sitemap or generate static sitemap |
| P1 | Meta | Missing og:image | /providers | Add defineOgImage() or useSeoMeta({ ogImage }) |
| P2 | Schema | No Organization JSON-LD | Layout | Add useSchemaOrg in default layout |

Priority scale:
- **P0**: Blocks indexing or causes major ranking loss
- **P1**: Missing standard SEO elements, visible in search results
- **P2**: Optimization opportunities, nice-to-have improvements
- **P3**: Minor polish, marginal gains

## Instructions

seo-expert

## Tags

seo, meta, schema, sitemap, performance
