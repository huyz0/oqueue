import java.time.Duration;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Properties;
import java.util.Set;
import java.util.TreeSet;
import org.apache.kafka.clients.consumer.KafkaConsumer;
import org.apache.kafka.clients.producer.KafkaProducer;
import org.apache.kafka.clients.producer.ProducerRecord;
import org.apache.kafka.common.TopicPartition;
import org.apache.kafka.common.serialization.ByteArrayDeserializer;
import org.apache.kafka.common.serialization.ByteArraySerializer;

/**
 * Java-client consumer-group conformance ({@code M4.17}, FR-20).
 *
 * <p>The reference implementation's half of {@code rdkafka_groups.py}: the
 * same three membership changes FR-20 names — an initial join, a consumer
 * added, and a consumer removed — each asserting that the group's partitions
 * are spread across the live members with no partition owned by two at once.
 * Running both clients matters because they are not interchangeable: their
 * assignors are separate implementations of the same protocol, and a broker
 * can satisfy one while starving the other.
 *
 * <p>⚠️ <b>Several single-partition topics, not one multi-partition topic.</b>
 * {@code Cluster::ensure_topic} creates every topic with exactly one
 * partition and this broker dispatches no {@code CreateTopics}, so three
 * topics are what make "spread without duplicates" a claim with content.
 *
 * <p>Prints {@code GROUP CONFORMANCE OK} on success.
 */
public final class GroupRebalance {
    private static final List<String> TOPICS =
            Arrays.asList("jgroups-a", "jgroups-b", "jgroups-c");
    private static final String GROUP = "m4-17-java";
    /**
     * ⚠️ <b>Shorter than either eviction path below, and {@code M4.38} is
     * why.</b> This was 60 s against a 6 s session timeout and a 20 s poll
     * interval, so the "a consumer is removed" stage was satisfied by any
     * path that eventually removed the member rather than by the
     * {@code LeaveGroup} that {@code close()} sends — and {@code M4.10} is
     * specifically {@code LeaveGroup} firing exactly one rebalance. Its
     * librdkafka sibling was measured doing precisely that: abandoning the
     * consumer instead of closing it still printed
     * {@code GROUP CONFORMANCE OK}.
     */
    private static final long SETTLE_MILLIS = 30_000;

    private static KafkaConsumer<byte[], byte[]> consumer(String bootstrap, String name) {
        Properties p = new Properties();
        p.put("bootstrap.servers", bootstrap);
        p.put("group.id", GROUP);
        p.put("client.id", name);
        p.put("auto.offset.reset", "earliest");
        p.put("enable.auto.commit", "false");
        // ⚠️ Past SETTLE_MILLIS on purpose -- see its own note.
        p.put("session.timeout.ms", "45000");
        p.put("heartbeat.interval.ms", "2000");
        // ⚠️ Seeds `rebalance_timeout_ms` on the wire, and — like the
        // session timeout above — deliberately past SETTLE_MILLIS, which is
        // the whole of M4.38. ⚠️ **Do not restore a short value here.** The
        // comment that stood in this place said it was "kept modest so a
        // round that never closes fails this harness rather than hanging
        // it"; that is SETTLE_MILLIS' job, and the Java client (unlike
        // librdkafka, which refuses max.poll.interval.ms < session.timeout.ms
        // outright) will happily accept a 20 s value here — which puts an
        // eviction path back inside the settle window and makes the removal
        // stage vacuously satisfiable again. Found by review of M4.38.
        p.put("max.poll.interval.ms", "300000");
        p.put(
                "partition.assignment.strategy",
                "org.apache.kafka.clients.consumer.CooperativeStickyAssignor");
        p.put("key.deserializer", ByteArrayDeserializer.class.getName());
        p.put("value.deserializer", ByteArrayDeserializer.class.getName());
        return new KafkaConsumer<>(p);
    }

    /**
     * Polls every consumer until the assignment is complete and disjoint.
     *
     * <p>⚠️ <b>Every consumer must be polled throughout.</b> The Java client
     * runs its rebalance on the caller's own {@code poll} thread, so a
     * consumer nobody polls never completes a join and is eventually evicted
     * — the group would then "settle" at the wrong size, against a state this
     * harness's own inattention created.
     */
    private static Map<String, Set<TopicPartition>> settle(
            Map<String, KafkaConsumer<byte[], byte[]>> consumers,
            String stage,
            int expectMembers) {
        long deadline = System.currentTimeMillis() + SETTLE_MILLIS;
        Map<String, Set<TopicPartition>> last = new LinkedHashMap<>();
        int samples = 0;
        while (System.currentTimeMillis() < deadline) {
            for (KafkaConsumer<byte[], byte[]> c : consumers.values()) {
                c.poll(Duration.ofMillis(200));
            }
            last = new LinkedHashMap<>();
            for (Map.Entry<String, KafkaConsumer<byte[], byte[]>> e : consumers.entrySet()) {
                last.put(e.getKey(), new HashSet<>(e.getValue().assignment()));
            }
            // ⚠️ Checked on every pass, not only once settled — see
            // `assertDisjoint`. FR-20's invariant is about every instant of a
            // rebalance, and the converged state is the one moment it cannot
            // be violated.
            assertDisjoint(stage + " (mid-rebalance)", last, false);
            samples++;
            Set<TopicPartition> union = new HashSet<>();
            int total = 0;
            int withWork = 0;
            for (Set<TopicPartition> tps : last.values()) {
                union.addAll(tps);
                total += tps.size();
                if (!tps.isEmpty()) {
                    withWork++;
                }
            }
            int wanted = Math.min(expectMembers, TOPICS.size());
            if (total == union.size() && union.size() == TOPICS.size() && withWork == wanted) {
                System.out.println(stage + ": settled after " + samples + " sample(s)");
                return last;
            }
        }
        throw new AssertionError(
                "group never settled within "
                        + SETTLE_MILLIS
                        + "ms with "
                        + expectMembers
                        + " member(s); last assignment "
                        + last);
    }

    /**
     * No partition owned twice — and, once settled, none owned by nobody.
     *
     * <p>{@code complete=false} is the mid-rebalance form: a partition
     * legitimately belongs to nobody between its owner revoking it and its
     * new owner being assigned it, which is what cooperative rebalancing
     * buys. Owning it <b>twice</b> is never legitimate, at any instant.
     */
    private static void assertDisjoint(
            String stage, Map<String, Set<TopicPartition>> assignment, boolean complete) {
        Map<TopicPartition, String> seen = new HashMap<>();
        for (Map.Entry<String, Set<TopicPartition>> e : assignment.entrySet()) {
            for (TopicPartition tp : e.getValue()) {
                String other = seen.put(tp, e.getKey());
                if (other != null) {
                    throw new AssertionError(
                            stage
                                    + ": "
                                    + tp
                                    + " assigned to both "
                                    + other
                                    + " and "
                                    + e.getKey()
                                    + " — FR-20's revoke-before-reassign invariant violated");
                }
            }
        }
        if (!complete) {
            return;
        }
        for (String topic : TOPICS) {
            TopicPartition tp = new TopicPartition(topic, 0);
            if (!seen.containsKey(tp)) {
                throw new AssertionError(stage + ": " + tp + " assigned to nobody");
            }
        }
        // ⚠️ No `Stream.toList()` here: it needs JDK 16+, while `RoundTrip.java`
        // — this harness's other Java leg — compiles under 11, and a `javac`
        // failure in this leg is reported as a `fail`, not a `skip`. Raising
        // the JDK floor for a debug print would turn an older-JDK machine
        // from "one leg skipped" into "the harness failed". Found by review.
        Map<String, Object> pretty = new LinkedHashMap<>();
        for (Map.Entry<String, Set<TopicPartition>> e : assignment.entrySet()) {
            Set<String> names = new TreeSet<>();
            for (TopicPartition tp : e.getValue()) {
                names.add(tp.toString());
            }
            pretty.put(e.getKey(), names);
        }
        System.out.println(stage + ": " + pretty);
    }

    public static void main(String[] args) throws Exception {
        String bootstrap = args[0];

        Properties pp = new Properties();
        pp.put("bootstrap.servers", bootstrap);
        pp.put("delivery.timeout.ms", "10000");
        pp.put("request.timeout.ms", "5000");
        pp.put("key.serializer", ByteArraySerializer.class.getName());
        pp.put("value.serializer", ByteArraySerializer.class.getName());
        try (KafkaProducer<byte[], byte[]> producer = new KafkaProducer<>(pp)) {
            for (String topic : TOPICS) {
                for (int i = 0; i < 3; i++) {
                    producer.send(new ProducerRecord<>(topic, (topic + "-" + i).getBytes())).get();
                }
            }
        }
        System.out.println("produced to " + TOPICS.size() + " topics");

        Map<String, KafkaConsumer<byte[], byte[]>> consumers = new LinkedHashMap<>();
        try {
            for (String name : new String[] {"c1", "c2"}) {
                KafkaConsumer<byte[], byte[]> c = consumer(bootstrap, name);
                c.subscribe(TOPICS);
                consumers.put(name, c);
            }
            assertDisjoint("join", settle(consumers, "join", 2), true);

            KafkaConsumer<byte[], byte[]> c3 = consumer(bootstrap, "c3");
            c3.subscribe(TOPICS);
            consumers.put("c3", c3);
            assertDisjoint("added", settle(consumers, "added", 3), true);

            // ⚠️ close() sends LeaveGroup, and that is the whole of what
            // this stage tests: both eviction timeouts are past
            // SETTLE_MILLIS, so a member that merely stopped polling would
            // still be in the group when settle gives up.
            consumers.remove("c2").close();
            assertDisjoint("removed", settle(consumers, "removed", 2), true);
        } finally {
            for (KafkaConsumer<byte[], byte[]> c : consumers.values()) {
                try {
                    c.close(Duration.ofSeconds(5));
                } catch (RuntimeException ignored) {
                    // cleanup must not mask a failure
                }
            }
        }

        System.out.println("GROUP CONFORMANCE OK");
    }
}
