## FAQ

<details>
<summary>What is Arbstr?</summary>

Arbstr is an intelligent local proxy and cost arbitrage engine for AI inference. It acts as a drop-in OpenAI-compatible router that automatically selects the cheapest qualified provider for each request (local, standard, or frontier) and, with vault enabled, bills each request in Bitcoin sats against an agent's balance.

Think of it as "NiceHash for LLM inference": it routes across Routstr, OpenRouter, Anthropic, local models, or any OpenAI-compatible endpoint.

</details>

<details>
<summary>How does the routing work?</summary>

1. Your app sends requests to Arbstr (same as OpenAI API).
2. Arbstr applies policies and complexity heuristics.
3. It selects the cheapest available provider that satisfies the request.
4. Forwards the request and streams the response back.
5. If vault billing is enabled, it reserves → settles → releases sats automatically.

It supports local models (Ollama, mesh-llm), Routstr providers, and any OpenAI-compatible endpoint.

</details>

<details>
<summary>Does it require Bitcoin payments?</summary>

**No — it's optional.**

- Without a vault configured → runs as a **free proxy**.
- With vault → every request is billed to an agent account. Vault runs in simulated mode by default (balances only, no real funds); switch its wallet backend to Lightning or Cashu for real sats.

You can start in free mode and enable payments later.

</details>

<details>
<summary>What providers can I use?</summary>

- Local: Ollama, mesh-llm, or any local OpenAI-compatible server
- Cloud: Routstr providers, OpenRouter, Anthropic (`https://api.anthropic.com/v1`), and other OpenAI-compatible APIs
- Live market quotes via the optional `[tokenstats]` feed

Providers with `auto_discover = true` automatically pull available models at startup.

</details>

<details>
<summary>What is the vault / billing system?</summary>

[arbstr-vault](https://github.com/johnzilla/arbstr-vault) holds a balance and a spending policy for each agent. For each request arbstr:

- Reserves an estimated amount upfront (vault refuses it if it breaks the agent's per-request or daily limit)
- Settles the actual cost after the response
- Releases the reservation if the request fails

Vault also releases reservations that are never settled, after 15 minutes. Full stack available via [arbstr-node](https://github.com/johnzilla/arbstr-node).

</details>

<details>
<summary>Is Arbstr production-ready?</summary>

It is feature-rich, with good observability, circuit breakers, streaming support and CI on every change. However, it is still early-stage — configuration and APIs may evolve. Suitable for personal use and experimentation; audit before high-volume or critical deployments.

</details>

<details>
<summary>How do I run the full stack?</summary>

The easiest way is using **arbstr-node**:

```bash
git clone https://github.com/johnzilla/arbstr-node
cd arbstr-node
cp .env.example .env   # fill in secrets
docker compose up -d
```

Then create an agent, fund it and set its spending policy — the [arbstr-node README](https://github.com/johnzilla/arbstr-node#create-an-agent) walks through it.

</details>
