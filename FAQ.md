## FAQ

<details>
<summary>What is Arbstr?</summary>

Arbstr is an intelligent local proxy and cost arbitrage engine for AI inference. It acts as a drop-in OpenAI-compatible router that automatically selects the cheapest qualified provider for each request (local, standard, or frontier) and settles payment in Bitcoin sats via Lightning.

Think of it as "NiceHash for LLM inference" — part of the Routstr decentralized marketplace.

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
- With vault + Lightning → enables real sats-based arbitrage and billing.

You can start in free mode and enable payments later.

</details>

<details>
<summary>What providers can I use?</summary>

- Local: Ollama, mesh-llm, or any local OpenAI-compatible server
- Cloud: Routstr marketplace providers
- Any custom OpenAI-compatible APIs

Providers with `auto_discover = true` automatically pull available models at startup.

</details>

<details>
<summary>What is the vault / billing system?</summary>

The optional vault integrates with Lightning (LND) and Cashu. For each request it:

- Reserves an estimated amount upfront
- Settles the actual cost after the response
- Releases any unused reservation on failure

All settled in Bitcoin sats. Full stack available via [arbstr-node](https://github.com/johnzilla/arbstr-node).

</details>

<details>
<summary>Is Arbstr production-ready?</summary>

It is stable and feature-rich (v2.0 shipped), with good observability, circuit breakers, and streaming support. However, it is still labeled early-stage — configuration and APIs may evolve. Suitable for personal use and experimentation; audit before high-volume or critical deployments.

</details>

<details>
<summary>How do I run the full stack?</summary>

The easiest way is using **arbstr-node**:

```bash
git clone https://github.com/johnzilla/arbstr-node
cd arbstr-node
cp .env.example .env
docker compose up
