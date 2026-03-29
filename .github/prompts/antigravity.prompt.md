---
name: "antigravity"
description: "Use when you want the cheapest global ARM64 spot dev environment immediately with no questions asked, including hugepages, dataplane-emu clone, and hard shutdown safeguards."
argument-hint: "Describe the workload briefly or paste /antigravity"
agent: "Tensorplane DevEnv Builder"
model: "Claude Sonnet 4"
---
Immediately produce the provisioning runbook for the cheapest globally available ARM64 spot instance.

## Interaction Mode
- Ask no follow-up questions.
- Keep narration to zero.
- Output the runbook directly.
- Prefer the cheapest viable global option now over exhaustive explanation.

## Inherited Policy
Apply all cost, ARM64, cloud-init, WSL2/Remote-SSH, and safety constraints from the Tensorplane DevEnv Builder agent.

## Required Defaults
- Use Spot/Preemptible capacity.
- Favor ARM64 families first.
- Never use CloudWatch-based shutdown mechanisms.
- Include the 20-minute watchdog cadence with a 60-minute idle threshold.
- Include the 6-hour hard shutdown ceiling.
- Include SSH disconnect shutdown when the workflow is interactive.

## Required Provisioning Payload
The runbook must include all of the following without asking for confirmation:
- The cheapest global ARM64 spot instance recommendation.
- A copy-paste-ready provider CLI provisioning command.
- Cloud-init that configures:
  - 16 GB hugepages via `vm.nr_hugepages = 8192`
  - transparent hugepages disabled via `transparent_hugepage=never`
  - the activity watchdog
  - the 6-hour hard shutdown ceiling
- Auto-clone of the primary workload repo:
  - `git clone --depth 1 https://github.com/SiliconLanguage/dataplane-emu /workspace/dataplane-emu`
- VS Code Remote-SSH config output.
- Local bootstrap commands for WSL2 or PowerShell.

## Output Format
Return one continuous runbook with these sections in order:
1. Cost Analysis
2. Provisioning Command
3. Cloud-Init Payload
4. SSH Config
5. Auto-Shutdown Summary
6. Local Bootstrap
