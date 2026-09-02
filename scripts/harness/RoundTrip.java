import java.time.Duration;
import java.util.Collections;
import java.util.List;
import java.util.Properties;
import java.util.concurrent.Future;
import org.apache.kafka.clients.consumer.ConsumerRecord;
import org.apache.kafka.clients.consumer.ConsumerRecords;
import org.apache.kafka.clients.consumer.KafkaConsumer;
import org.apache.kafka.clients.producer.KafkaProducer;
import org.apache.kafka.clients.producer.ProducerRecord;
import org.apache.kafka.clients.producer.RecordMetadata;
import org.apache.kafka.common.TopicPartition;
import org.apache.kafka.common.serialization.ByteArrayDeserializer;
import org.apache.kafka.common.serialization.ByteArraySerializer;

public final class RoundTrip {
    public static void main(String[] args) throws Exception {
        String bootstrap = args[0];
        String topic = "harness-java";
        byte[][] payloads = {"one".getBytes(), "two".getBytes(), "three".getBytes()};

        // ⚠️ `enable.idempotence`/`acks` are left at the client's own
        // defaults (`M11.9`) — `true`/`all` since KIP-679, and no longer
        // forced to `false`/`1` now that `InitProducerId` exists
        // (`M11.4`-`M11.8`). Setting `acks` explicitly to anything but
        // `all` while idempotence defaults on would throw a
        // `ConfigException` at construction, which is exactly the
        // incompatibility a real, unconfigured client never hits.
        Properties pp = new Properties();
        pp.put("bootstrap.servers", bootstrap);
        pp.put("delivery.timeout.ms", "10000");
        pp.put("request.timeout.ms", "5000");
        pp.put("key.serializer", ByteArraySerializer.class.getName());
        pp.put("value.serializer", ByteArraySerializer.class.getName());
        try (KafkaProducer<byte[], byte[]> producer = new KafkaProducer<>(pp)) {
            for (byte[] p : payloads) {
                Future<RecordMetadata> f = producer.send(new ProducerRecord<>(topic, p));
                RecordMetadata m = f.get();
                System.out.println("PRODUCED partition=" + m.partition() + " offset=" + m.offset());
            }
        }

        Properties cp = new Properties();
        cp.put("bootstrap.servers", bootstrap);
        cp.put("group.id", "harness-java");
        cp.put("enable.auto.commit", "false");
        cp.put("auto.offset.reset", "earliest");
        cp.put("key.deserializer", ByteArrayDeserializer.class.getName());
        cp.put("value.deserializer", ByteArrayDeserializer.class.getName());
        int got = 0;
        long deadline = System.currentTimeMillis() + 20000;
        try (KafkaConsumer<byte[], byte[]> consumer = new KafkaConsumer<>(cp)) {
            TopicPartition tp = new TopicPartition(topic, 0);
            consumer.assign(Collections.singletonList(tp));
            consumer.seek(tp, 0);
            while (got < payloads.length && System.currentTimeMillis() < deadline) {
                ConsumerRecords<byte[], byte[]> records = consumer.poll(Duration.ofSeconds(1));
                for (ConsumerRecord<byte[], byte[]> r : records) {
                    String expected = new String(payloads[got]);
                    String actual = new String(r.value());
                    if (!expected.equals(actual)) {
                        throw new IllegalStateException("payload " + got + ": " + actual);
                    }
                    got++;
                }
            }
        }
        if (got != payloads.length) {
            throw new IllegalStateException("consumed " + got + " of " + payloads.length);
        }
        System.out.println("ROUND TRIP OK");
    }
}
