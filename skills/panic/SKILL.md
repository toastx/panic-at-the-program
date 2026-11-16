---
name: panic
description: Diagnose failed Solana transactions with the `panic` CLI, which returns JSON with the error's name and meaning, the failing instruction, the accounts at fault, a fix, and an `action` saying what to do next. Use whenever a Solana transaction or a Solana/Anchor test fails ("custom program error: 0x…", "Transaction simulation failed", AnchorError, "exceeded CUs", "insufficient lamports", "already in use", missing signature), and before sending a transaction you are unsure of, to simulate it first. Works on localnet, devnet, testnet and mainnet.
---

# panic: diagnose Solana transaction failures

`panic` reads a failed transaction, or simulates an unsent one, and says what went wrong and what to do about it. Use it before reading raw program logs yourself. It names error codes from the Anchor, SPL and runtime tables and from the program's IDL. It also checks the failing instruction's accounts against the IDL (signers, writability, PDA seeds, owners, rent), and it picks a next step.

## Setup

Check with `command -v panic`. If it is missing, ask the user before installing it:

```sh
curl -fsSL https://raw.githubusercontent.com/toastx/panic-at-the-program/master/install.sh | sh
```

## Commands

```sh
panic diagnose <signature> [cluster] --json      # a transaction that failed on-chain
panic simulate <transaction> [cluster] --json    # an unsent transaction, base64 or base58; it need not be signed
```

- `cluster` is `localnet`, `devnet` (the default), `testnet`, `mainnet`, a name saved with `panic rpc add`, or an RPC URL.
- Run it from the project root, so the program's IDL in `target/idl` and any names in `patp.toml` are picked up.
- Do not run bare `panic`, `panic <signature>` or `panic test`. They open an interactive TUI and need a real terminal.

## Getting something to diagnose from a failing test

- **The error carries a transaction signature:** run `panic diagnose <signature> localnet --json`, using the cluster the test ran against.
- **The validator must still be up.** `anchor test` stops its validator on exit, so do one of these:
  - rerun with `anchor test --detach`
  - start `solana-test-validator` yourself and run `anchor test --skip-local-validator`
- **"Transaction simulation failed" means preflight rejected it**, so it never landed and has no signature. Do one of these:
  - Serialize it in the test and simulate it: `tx.serialize({ requireAllSignatures: false, verifySignatures: false }).toString("base64")` for a legacy transaction, or `Buffer.from(tx.serialize()).toString("base64")` for a versioned one. Then run `panic simulate <that> localnet --json`.
  - Or send it with `skipPreflight: true`, so it fails on-chain and gets a signature you can diagnose.

## Reading the result

```json
{
  "status": "failed",
  "diagnostic": "cu.exceeded",
  "confidence": "high",
  "action": "raise_compute_and_retry",
  "retryable": true,
  "error": { "id": "runtime.ComputationalBudgetExceeded", "name": "...", "meaning": "...", "account": null, "source": null },
  "instruction": "swap", "program": "<program id>",
  "fix": "ran out of compute: 200000 of 200000 units used — raise it with ComputeBudget::SetComputeUnitLimit",
  "accounts": [ { "index": 0, "name": "vault", "key": "...", "signer": false, "writable": true, "findings": [] } ],
  "notes": [ { "id": "...", "tier": "fix", "confidence": "high", "text": "...", "expected": "...", "actual": "..." } ],
  "compute": { "used": 200000, "limit": 200000 },
  "logs": [ "...the failing instruction's logs..." ]
}
```

Read it in this order: `action`, then `fix`, then any `accounts[].findings`. A finding names the exact account, and the note behind it carries `expected` against `actual`. `error.source` is the file and line where the program threw, and `error.account` is the account an Anchor constraint rejected.

| `action` | What to do |
|---|---|
| `retry` | Resend unchanged, with a fresh blockhash. Retry a few times at most. |
| `raise_compute_and_retry` | Add `ComputeBudgetProgram.setComputeUnitLimit` about 20% above `compute.used`. If it is your own program and the usage is unexpected, cut the work in that instruction instead. |
| `requote_and_retry` | Fetch a fresh quote or price, rebuild, and resend. Widen the slippage tolerance only if the user allows it. |
| `fund_account` | On localnet or devnet, airdrop to the account or fund it. On mainnet, stop and ask the user. Never move the user's funds on your own. |
| `stop_and_report` | Do not resend. Tell the user what failed, such as which signer is missing. |
| `fix_transaction` | The transaction or the program is wrong. Edit the code using `fix`, `error.account`, `error.source`, the findings, and the notes. |
| `send` | Only from `simulate`: the transaction succeeded in simulation and is safe to send. |

After a change, diagnose or simulate again to confirm the failure is gone, rather than assuming it is.

## Confidence

- `high`: from an error table, the IDL, or on-chain account state.
- `medium`: inferred from the error's name. Check it against the program's source before relying on it.
- `low`: a custom code that panic could not name, because the program's IDL is not available.
  - For your own program, run `anchor build` so `target/idl` exists, then diagnose again.
  - For someone else's, look the code up in their error enum. Anchor user errors start at 6000, so `0x1771` = 6001 = the second variant. Do not guess.

If the output has `"status": "error"`, panic could not fetch or simulate the transaction. Usually the signature is on a different cluster, or the validator has stopped. Check those before anything else.
