db = db.getSiblingDB("db");

db.createCollection("urls");

db.createCollection("visits", {
  timeseries: {
    timeField: "timestamp",
    metaField: "id",
    granularity: "seconds",
    bucketRoundingSeconds: 60,
    bucketMaxSpanSeconds: 60
  },
  expireAfterSeconds: 300
});
