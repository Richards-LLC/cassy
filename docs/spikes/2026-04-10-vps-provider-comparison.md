# VPS Provider Comparison for Cassy Remote Server

**Date:** 2026-04-10
**Status:** Recommendation ready
**Task:** cas-4dfc

## Context

The Cassy remote server runs Cassy binary + Claude Code + Node.js Slack bridge on a single Linux box. The workload is mostly I/O-bound (Claude API calls), not CPU-bound. The current Vultr box (2 vCPU / 3.8GB) is underpowered. We need 4+ vCPU, 8-16GB RAM, 100GB+ NVMe, US datacenter, SSH access, and multi-user Linux support.

## Requirements

| Requirement | Weight |
|---|---|
| 4+ vCPU, 8-16GB RAM, 100GB+ NVMe | Must have |
| US datacenter (low latency to Anthropic API) | Must have |
| SSH access | Must have |
| US-based account creation (payment, billing, KYC) | Must have |
| Multi-user Linux (two user accounts) | Must have |
| Reliable uptime | Must have |
| Cost efficiency | Strong preference |

## Provider Comparison

### Shared vCPU Plans (4 vCPU / 8GB RAM tier)

| Provider | Plan | vCPU | RAM | Storage | Bandwidth | Price/mo | Notes |
|---|---|---|---|---|---|---|---|
| **Hetzner** | CPX32 | 4 shared | 8 GB | 160 GB NVMe | ~2 TB | **$15.59** | Best price by far |
| **Hetzner** | CPX42 | 8 shared | 16 GB | 320 GB NVMe | ~4 TB | **$28.59** | Double specs, still cheap |
| **Vultr** | Regular Perf | 4 shared | 8 GB | 160 GB SSD | 4 TB | $40.00 | Previous-gen Intel, SSD not NVMe |
| **Vultr** | High Perf | 4 shared | 8 GB | 180 GB NVMe | 6 TB | $48.00 | AMD EPYC / NVMe |
| **Vultr** | High Freq | 4 shared | 8 GB | 160 GB NVMe | 4 TB | $48.00 | 3GHz+ Intel Xeon |
| **DigitalOcean** | s-4vcpu-8gb | 4 shared | 8 GB | 160 GB SSD | 5 TB | $48.00 | Standard tier |
| **Linode** | Shared 8GB | 4 shared | 8 GB | 160 GB SSD | 5 TB | $48.00 | Akamai network |

### Dedicated vCPU Plans (4 vCPU tier)

| Provider | Plan | vCPU | RAM | Storage | Bandwidth | Price/mo | Notes |
|---|---|---|---|---|---|---|---|
| **Hetzner** | CCX23 | 4 dedicated | 16 GB | 160 GB NVMe | ~2 TB | **$35.59** | Best dedicated value |
| **Hetzner** | CCX33 | 8 dedicated | 32 GB | 240 GB NVMe | ~4 TB | **$70.59** | Overkill but still cheap |
| **Linode** | Dedicated 8GB | 4 dedicated | 8 GB | 160 GB SSD | 5 TB | $72.00 | |
| **DigitalOcean** | c-4 (CPU-Opt) | 4 dedicated | 8 GB | 50 GB SSD | 5 TB | $80.00 | Only 50GB storage |
| **Vultr** | VX1 4vCPU | 4 dedicated | 16 GB | Block Storage | 6 TB | ~$87.60 | $0.120/hr, boots from block storage |
| **DigitalOcean** | g-4vcpu-16gb | 4 dedicated | 16 GB | 50 GB SSD | 5 TB | $120.00 | Only 50GB storage |
| **Vultr** | General Purpose | 4 dedicated | 16 GB | 80 GB NVMe | 6 TB | $120.00 | |

## US Datacenter Availability

| Provider | US Regions |
|---|---|
| **Vultr** | Atlanta, Chicago, Dallas, Honolulu, LA, Miami, NJ, Seattle, Silicon Valley (9 US locations) |
| **DigitalOcean** | NYC (NYC1/2/3), SFO (SFO2/3) |
| **Linode (Akamai)** | Newark, Atlanta, Dallas, Fremont, Chicago, Seattle, Washington DC, Miami, Los Angeles |
| **Hetzner** | Ashburn VA, Hillsboro OR (2 US locations) |

## SSH Access

All four providers include full root SSH access on all Linux plans. No restrictions.

## US Account Creation

| Provider | US Account OK? | Payment Methods | KYC Notes |
|---|---|---|---|
| **Vultr** | Yes | Credit/debit card, PayPal, crypto, wire | Straightforward signup, US-headquartered company |
| **DigitalOcean** | Yes | Credit/debit card, PayPal | US-headquartered, no special restrictions |
| **Linode (Akamai)** | Yes | Credit/debit card, PayPal | US-headquartered (Akamai), no special restrictions |
| **Hetzner** | Yes, with caveats | Credit card, PayPal, bank transfer | German company with strict KYC. ID verification required. Name on card must match ID exactly. Avoid VPN/public WiFi during signup. Some users report verification delays |

## Gotchas

### Hetzner
- **Bandwidth slashed for US locations (Dec 2024):** Hetzner cut US included traffic by ~88-95% (e.g., CPX plans went from 20TB to 1-4TB). Overage at ~$1.19/TB. For our I/O-bound workload (API calls, SSH), 2TB/mo is likely sufficient but worth monitoring.
- **Strict KYC:** Signup can be rejected for name mismatches, VPN use, or free email addresses. Use real identity, home internet, and a non-free email.
- **EUR-based pricing:** Prices shown are approximate USD conversions. Actual charges depend on EUR/USD exchange rate at billing time. Account currency is locked at creation.
- **Fewer US regions:** Only 2 US locations vs 9 for Vultr. Ashburn VA is ideal (close to Anthropic API endpoints).

### Vultr
- **Shared plans use previous-gen Intel (Regular Performance):** The $40 tier is slower hardware. High Performance ($48) uses AMD EPYC/NVMe.
- **VX1 boots from block storage:** No local NVMe, relies on network-attached block storage. Adds latency for disk I/O.
- **Automatic backups cost 20% extra.**

### DigitalOcean
- **Low storage on dedicated plans:** CPU-Optimized and General Purpose only include 50GB SSD. Need to add block storage ($10/100GB/mo) to meet the 100GB+ requirement, increasing effective cost.
- **Per-second billing (Jan 2026):** Monthly cap at 672 hours (28 days).

### Linode (Akamai)
- **SSD, not NVMe:** Standard plans use SSD storage, not NVMe. Slightly slower disk I/O.
- **Shared CPU throttling:** Shared plans should stay below 80% sustained CPU usage.

## Price Comparison Summary (4 vCPU / 8GB tier)

```
Shared vCPU:
  Hetzner CPX32    $15.59/mo  ████
  Vultr Regular    $40.00/mo  ██████████
  Vultr High Perf  $48.00/mo  ████████████
  DigitalOcean     $48.00/mo  ████████████
  Linode           $48.00/mo  ████████████

Dedicated vCPU:
  Hetzner CCX23    $35.59/mo  █████████  (4 vCPU / 16GB — more RAM)
  Linode Ded 8GB   $72.00/mo  ██████████████████
  DO CPU-Opt c-4   $80.00/mo  ████████████████████  (only 50GB storage)
  Vultr GP 4vCPU  $120.00/mo  ██████████████████████████████
```

**Hetzner is 3x cheaper than the field for shared, and 2-3.4x cheaper for dedicated.**

## Recommendation

**Hetzner Cloud CCX23 — 4 dedicated vCPU / 16 GB RAM / 160 GB NVMe — ~$35.59/mo (Ashburn VA)**

### Why this plan

1. **Best value by a wide margin.** At $35.59/mo, the CCX23 with *dedicated* vCPUs costs less than a *shared* 4-vCPU plan at any other provider ($40-48/mo). That's dedicated CPU for 26% less than everyone else's shared CPU.

2. **Dedicated vCPUs eliminate noisy-neighbor risk.** While our workload is I/O-bound today, dedicated cores ensure consistent performance for Claude Code compilation, Cassy binary builds, and any future CPU-intensive tasks.

3. **16 GB RAM (double the minimum).** Comfortably runs Cassy + Claude Code + Node.js Slack bridge + OS overhead with room to grow. Other providers charge $72-120/mo for 4 dedicated vCPU + 16GB.

4. **160 GB NVMe meets storage requirement.** Sufficient for Cassy binary, project repos, logs, and tooling. Can be expanded with Hetzner volumes if needed.

5. **Ashburn VA datacenter.** Low latency to Anthropic API endpoints on the US East Coast.

6. **2 TB included bandwidth is sufficient.** Our workload is API calls (small JSON payloads) and SSH sessions. 2 TB/mo is more than enough. If we ever need more, overage is ~$1.19/TB.

### Why not others

- **Vultr dedicated ($120/mo):** 3.4x more expensive for equivalent specs. Even Vultr shared ($48/mo) costs more than Hetzner dedicated.
- **DigitalOcean dedicated ($80-120/mo):** 2-3x more expensive, and only includes 50GB storage (would need $10/mo block storage addon).
- **Linode dedicated ($72/mo):** 2x more expensive, SSD not NVMe, same 8GB RAM.
- **Vultr shared ($48/mo):** Costs more than Hetzner dedicated. No dedicated CPU guarantee.

### Risk mitigation

- **KYC friction:** Create the Hetzner account from a home/office IP, use a non-free email, have a passport or government ID ready. Allow 1-2 business days for verification.
- **Bandwidth monitoring:** Set up alerts at 80% of the 2TB cap. If usage approaches the limit, overage is cheap ($1.19/TB) or upgrade to CPX42/CCX33 for more included traffic.
- **Fallback:** If Hetzner account creation fails or service is unsatisfactory, Vultr High Performance shared (4 vCPU/8GB/180GB NVMe, $48/mo) is the next best option — we already have a Vultr account.

### Monthly savings vs current assumption

| Scenario | Monthly Cost | Annual Cost | vs Vultr Dedicated ($120/mo) |
|---|---|---|---|
| Hetzner CCX23 (recommended) | $35.59 | $427 | **Save $1,013/yr (70%)** |
| Vultr High Perf shared (fallback) | $48.00 | $576 | Save $864/yr (60%) |
| Vultr General Purpose dedicated | $120.00 | $1,440 | Baseline |

---

*Sources: [Hetzner Cloud Pricing](https://costgoat.com/pricing/hetzner), [Vultr Pricing](https://www.vultr.com/pricing/), [DigitalOcean Pricing](https://onedollarvps.com/pricing/digitalocean-pricing), [Linode/Akamai Pricing](https://www.linode.com/pricing/), [Hetzner US Bandwidth Changes](https://adriano.fyi/posts/hetzner-raises-prices-while-significantly-lowering-bandwidth-in-us/), [Hetzner Price Adjustment Docs](https://docs.hetzner.com/general/infrastructure-and-availability/price-adjustment/). All prices fetched 2026-04-10.*
