# Advanced URL Shortener Assignment

## 1. Architecture

### What tech stack did you use and why?

- Rust & Axum: Rust is my primary language, and I’m most comfortable and productive with it. Its performance and memory safety make it a great fit for building scalable systems. I used Axum as it is a thin layer on top of `hyper` and has well-designed API surface.
- MongoDB: Chosen for its flexible schema and built-in support for time-series data, which is useful for tracking visits over time. You can also use other time-series databases such as TimescaleDB or QuestDB or even implement the leaderboard manually. I chose MongoDB because it keeps things simple.
- Redis: Used both as a high-speed cache for redirects and as a message queue for async processing of write operations. This setup helps decouple response times from database performance.
- ObjectID + UUIDv5 + Base62: ObjectIDs are used as primary keys in MongoDB. UUIDv5 gives us deterministic link identifiers (same URL -> same hash), and Base62 encoding is applied to keep shortened URLs compact and readable.
- Asynchronous Queues for All Writes: All write operations (new links and visit events) go through a message queue. This approach keeps the user-facing endpoints fast by offloading slower database writes to background workers, which helps the system stay responsive even under heavy traffic.

### How would you scale this system to support 10M redirects/day?

- Horizontal Scaling of Axum Instances: Deploy multiple Axum instances behind a load balancer (e.g., NGINX, Envoy, or cloud-native solutions like AWS ALB) to distribute incoming traffic and ensure high availability.
- Redis Clustering: Replace the single Redis node with a Redis Cluster to avoid memory bottlenecks and improve availability and fault tolerance. Redis clustering allows scaling reads and writes across multiple nodes.
- MongoDB Sharding: If the dataset grows significantly, collections can be sharded across multiple MongoDB nodes. Sharding based on hashed URL or ObjectID ensures even data distribution.

### How does your leaderboard stay accurate and performant?

- Write Buffering: Each visit event is enqueued in Redis, ensuring that it doesn’t slow down the redirect path.
- Asynchronous Processing: A background worker processes visit events, updating the persistent store (MongoDB) and maintaining intermediate visit counts.
- Periodic Recalculation: Every 60 seconds, a worker recalculates the leaderboard based on the latest data and stores the result in Redis.
- Cached Reads: Leaderboard requests are served directly from Redis, providing sub-millisecond latency. Cold reads fall back to recalculating from the database and warming the cache.

## 2. Tradeoffs

### What consistency guarantees does your leaderboard offer?

The system prioritizes speed over strict consistency. The leaderboard is eventually consistent. There can be up to a 60-second delay between an actual visit and it showing up in the leaderboard. Real-time updates are not reflected immediately, which is acceptable for most leaderboard use-cases. However, accuracy is guaranteed in the long term since all events are eventually processed and persisted.

### What happens if Redis/Postgres goes down temporarily?
If Redis goes down:
- Redirects still work by falling back to MongoDB lookups.
- Leaderboard reads degrade in performance since they must be recalculated live from MongoDB instead of served from cache.

If MongoDB goes down:
- Cached redirects in Redis continue to function, ensuring users can still reach destinations.
- New URL creations are still accepted and cached temporarily; database writes are queued for retry.
- Leaderboard becomes stale since fresh data can't be persisted or recalculated accurately.

### Did you optimize for read throughput, write throughput, memory, or something else?

Read Throughput was the primary optimization target. In a URL shortener, the majority of traffic is read-heavy (i.e., users resolving short links). Caching redirects in Redis and using base62-encoded keys ensures fast lookups. Message queues for writes ensure that insert operations do not block the user-facing flow.

## 3. Limitations

### What parts of your system are not production-ready?

- Rate Limiting: No protection against spam or abuse for link creation. This should ideally be handled via a reverse proxy (e.g., Cloudflare Rules) or an API Gateway with built-in rate limits.
- Error Handling: While present, the current granularity and coverage could be improved, especially for async background jobs.
- Task Separation: Recurring jobs (leaderboard updates, queue processing) are currently embedded in the app process. In a production setup, these should be deployed as independent services (e.g., via containers or serverless functions).
- Redis as a Queue: Redis is being used as a makeshift message queue, which isn't ideal under heavy load or for durability. A cloud-native solution like Amazon SQS or Google Pub/Sub would provide better reliability, visibility, and scaling.
- Redis Cluster Compatibility: The Redis interaction logic assumes a single-node setup. To be production-ready, the logic should be adapted to support Redis Clusters.

### Potential Failure Modes Under Heavy Load

- Redis Overload: A spike in traffic can overwhelm Redis if it’s not clustered or properly scaled. This would affect both caching and queue processing, increasing latency and possibly losing queued writes if persistence isn’t enabled.
- MongoDB Write Lag: If MongoDB can’t keep up with the queue, the leaderboard fall behind, impacting freshness of user-facing metrics.
- Queue Bloat: If the background worker can’t keep up with Redis queue size, memory usage can spike, risking process crashes or OOM kills, especially on smaller instances.