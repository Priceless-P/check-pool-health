# Pool Health Check Script

This script checks the health of the pool by establishing mining and jd SV2  connections. It verifies that expected SV2 messages are received within a timeout window, ensuring the pool is operational.

## 🚀 Running the Script

You can run the health check using:

```bash
cargo run --release -- --pool <POOL_ADDRESS> [--pubkey <AUTH_PUBKEY>]
```

### Arguments

| Flag             | Description                                          | Required | Default                                               |
| ---------------- | ---------------------------------------------------- | -------- | ----------------------------------------------------- |
| `--pool`, `-p`   | Address of the mining pool  | Yes      | N/A                                                   |
| `--pubkey`, `-k` | Public key       | No       | `9au...H72` |


If all expected messages are received, the script prints:

```
Pool is healthy
```

Otherwise, it returns the list of missing messages and exits with a non-zero status code.
