---
name: producthunter
title: Product Hunt Daily Report
description: Fetch today's Product Hunt launches via GraphQL API and generate a daily digest report in Chinese.
trigger: When user asks for Product Hunt daily report, PH trending, or when scheduled task invokes PH daily digest.
platforms: [linux]
deps: [curl, jq]
version: 1.0.0
updated_at: 2026-03-09
---

# Product Hunt Daily Report Skill

Generate a daily digest of Product Hunt launches, sorted by votes, with Chinese commentary.

## Configuration

- **API Endpoint:** `https://api.producthunt.com/v2/api/graphql`
- **Auth:** Bearer token (stored below)
- **PH_ACCESS_TOKEN:** `IOsdE9GUWyXdlYRs5EeOFQzESbHqMUF3snQ-wkghP78`

## How PH Date Cycle Works

Product Hunt posts go live at **08:01 UTC** daily (~12:01 AM Pacific). To fetch "today's" products:
- Use `postedAfter` = yesterday at `00:00:00Z` (captures the current day's batch)
- Results are ordered by `VOTES` descending

## Fetching Products

Run this bash command to fetch today's top 20 PH launches:

```bash
POSTED_AFTER=$(date -u -d "yesterday 00:00:00" +%Y-%m-%dT%H:%M:%SZ)

curl -s -X POST https://api.producthunt.com/v2/api/graphql \
  -H "Authorization: Bearer IOsdE9GUWyXdlYRs5EeOFQzESbHqMUF3snQ-wkghP78" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "query { posts(first: 20, order: VOTES, postedAfter: \"'"$POSTED_AFTER"'\") { edges { node { name tagline votesCount commentsCount website url createdAt featuredAt topics { edges { node { name } } } } } } }"
  }' | jq '[.data.posts.edges[].node | {name, tagline, votesCount, commentsCount, url, topics: [.topics.edges[].node.name]}]'
```

## Output Format

Format the result as a Chinese daily report. Take the **Top 10** by votes:

```
🚀 Product Hunt 每日热榜 Top 10（YYYY-MM-DD）

1. 🏆 ProductName — ⬆️ votes | 💬 comments
   📝 Tagline
   🏷️ Topic1, Topic2 | 🔗 PH链接

2. ...

📊 今日洞察：[1-2句话总结当天趋势，如AI工具占比、新兴赛道等]
```

## URL Handling

The API returns tracking URLs like:
```
https://www.producthunt.com/products/xxx?utm_campaign=...
```

For cleaner display, strip UTM params or use the base product URL:
```
https://www.producthunt.com/products/xxx
```

## Error Handling

- If API returns auth error → token may have expired, notify user to refresh
- If `edges` is empty → PH day hasn't rolled over yet (before 08:01 UTC), use previous day's `postedAfter`
- Rate limit: PH API allows 450 requests/day on free tier, daily report uses 1 request

## Notes

- The `order: VOTES` parameter sorts by upvote count descending
- `first: 20` fetches top 20 to have buffer; display top 10
- `featuredAt` being non-null means the product was featured (editor's pick)
- Topics provide category context for trend analysis
