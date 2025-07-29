# Pool Health Check 

This library checks the health of the pool by establishing mining and jd SV2  connections. It verifies that expected SV2 messages are received within a timeout window, ensuring the pool is operational.

## 🚀 Using the Lib

You can run the health check using:

```rs
use check_pool_health::check;

#[tokio::main]
async fn main() {
    check("<pool-address:port>".to_string(), "<token>".to_string(), None).await.unwrap();
}
```

### Arguments

| Input             | Description                                          | Required | Default                                               |
| ---------------- | ---------------------------------------------------- | -------- | ----------------------------------------------------- |
| `pool`  | Address of the mining pool  | Yes      | N/A                                                   |
| `token` | Unique token | Yes | N/A
| `pubkey` | Public key       | No       | `9au...H72` |


If all expected messages are received, the script prints:

```
Pool is healthy
```

Otherwise, it returns the list of missing messages and exits.
