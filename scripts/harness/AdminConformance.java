import java.time.Duration;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collections;
import java.util.HashMap;
import java.util.Map;
import java.util.Properties;
import java.util.Set;
import java.util.concurrent.ExecutionException;
import org.apache.kafka.clients.admin.Admin;
import org.apache.kafka.clients.admin.Config;
import org.apache.kafka.clients.admin.ConfigEntry;
import org.apache.kafka.clients.admin.NewTopic;
import org.apache.kafka.clients.consumer.ConsumerConfig;
import org.apache.kafka.clients.consumer.KafkaConsumer;
import org.apache.kafka.common.KafkaFuture;
import org.apache.kafka.common.config.ConfigResource;
import org.apache.kafka.common.quota.ClientQuotaAlteration;
import org.apache.kafka.common.quota.ClientQuotaEntity;
import org.apache.kafka.common.quota.ClientQuotaFilter;
import org.apache.kafka.common.serialization.ByteArrayDeserializer;

/**
 * Real Kafka AdminClient coverage for M12.15.
 *
 * <p>The class deliberately uses futures returned by AdminClient rather than
 * the broker's wire fixtures. It covers mixed-success topic creation,
 * configuration read/write, group inspection, and principal quota updates.
 * The {@code denied} mode is used against a data-plane-only listener and must
 * fail an administrative request.
 */
public final class AdminConformance {
    private static final String RETENTION = "retention.ms";
    private static final String QUOTA = "in_flight_requests";

    private AdminConformance() {}

    private static void security(Properties properties) {
        String cert = System.getenv("OQUEUE_ADMIN_CERT");
        String principal = System.getenv("OQUEUE_ADMIN_PRINCIPAL");
        String password = System.getenv("OQUEUE_ADMIN_PASSWORD");
        if (cert == null || principal == null || password == null) {
            return;
        }
        try {
            properties.put("security.protocol", "SASL_SSL");
            properties.put("sasl.mechanism", "PLAIN");
            properties.put(
                    "sasl.jaas.config",
                    "org.apache.kafka.common.security.plain.PlainLoginModule required "
                            + "username=\"" + principal + "\" password=\"" + password + "\";");
            properties.put("ssl.truststore.type", "PEM");
            properties.put("ssl.truststore.certificates", Files.readString(Path.of(cert)));
            properties.put("ssl.endpoint.identification.algorithm", "");
        } catch (java.io.IOException error) {
            throw new IllegalStateException("could not read the test CA certificate", error);
        }
    }

    private static Admin admin(String bootstrap) {
        Properties properties = new Properties();
        properties.put("bootstrap.servers", bootstrap);
        properties.put("client.id", "m12-admin-client");
        properties.put("request.timeout.ms", "5000");
        properties.put("default.api.timeout.ms", "10000");
        security(properties);
        return Admin.create(properties);
    }

    private static void expectFailure(KafkaFuture<Void> future) throws Exception {
        try {
            future.get();
        } catch (ExecutionException error) {
            return;
        }
        throw new AssertionError("invalid CreateTopics unexpectedly succeeded");
    }

    private static void seedGroup(String bootstrap, String topic, String group) {
        Properties properties = new Properties();
        properties.put(ConsumerConfig.BOOTSTRAP_SERVERS_CONFIG, bootstrap);
        properties.put(ConsumerConfig.GROUP_ID_CONFIG, group);
        properties.put(ConsumerConfig.CLIENT_ID_CONFIG, "m12-admin-group-seed");
        properties.put(ConsumerConfig.KEY_DESERIALIZER_CLASS_CONFIG, ByteArrayDeserializer.class.getName());
        properties.put(ConsumerConfig.VALUE_DESERIALIZER_CLASS_CONFIG, ByteArrayDeserializer.class.getName());
        properties.put(ConsumerConfig.AUTO_OFFSET_RESET_CONFIG, "earliest");
        security(properties);
        try (KafkaConsumer<byte[], byte[]> consumer = new KafkaConsumer<>(properties)) {
            consumer.subscribe(Collections.singleton(topic));
            consumer.poll(Duration.ofSeconds(2));
        }
    }

    private static void runConformance(String bootstrap) throws Exception {
        String suffix = Long.toString(System.nanoTime());
        String primary = "m12-admin-primary-" + suffix;
        String invalid = "m12-admin-invalid-" + suffix;
        String group = "m12-admin-group";
        try (Admin admin = admin(bootstrap)) {
            Map<String, NewTopic> mixedTopics = new HashMap<>();
            mixedTopics.put(primary, new NewTopic(primary, 1, (short) 1));
            mixedTopics.put(invalid, new NewTopic(invalid, 1, (short) 2));
            Map<String, KafkaFuture<Void>> mixedResults = admin.createTopics(mixedTopics.values()).values();
            mixedResults.get(primary).get();
            expectFailure(mixedResults.get(invalid));

            Set<String> listed = admin.listTopics().names().get();
            if (!listed.contains(primary) || listed.contains(invalid)) {
                throw new AssertionError("created topics were not visible to ListTopics");
            }

            ConfigResource resource = new ConfigResource(ConfigResource.Type.TOPIC, primary);
            Config before = admin.describeConfigs(Collections.singleton(resource))
                    .all().get().get(resource);
            if (before.get(RETENTION) == null) {
                throw new AssertionError("DescribeConfigs omitted retention.ms");
            }
            admin.alterConfigs(Collections.singletonMap(
                    resource, new Config(Collections.singletonList(new ConfigEntry(RETENTION, "86400000")))))
                    .all().get();
            Config after = admin.describeConfigs(Collections.singleton(resource))
                    .all().get().get(resource);
            if (!"86400000".equals(after.get(RETENTION).value())) {
                throw new AssertionError("AlterConfigs did not persist retention.ms");
            }

            seedGroup(bootstrap, primary, group);
            boolean groupListed = admin.listConsumerGroups().all().get().stream()
                    .anyMatch(listing -> listing.groupId().equals(group));
            if (!groupListed) {
                throw new AssertionError("ListGroups omitted the seeded group");
            }
            if (!group.equals(admin.describeConsumerGroups(Collections.singleton(group))
                    .all().get().get(group).groupId())) {
                throw new AssertionError("DescribeGroups returned the wrong group");
            }

            for (String[] principalAndLimit : new String[][] {{"alice", "8.0"}, {"bob", "1.0"}}) {
                String principal = principalAndLimit[0];
                double limit = Double.parseDouble(principalAndLimit[1]);
                ClientQuotaAlteration.Op operation = new ClientQuotaAlteration.Op(QUOTA, limit);
                Map<String, String> entityMap = new HashMap<>();
                entityMap.put(ClientQuotaEntity.USER, principal);
                admin.alterClientQuotas(Collections.singleton(new ClientQuotaAlteration(
                        new ClientQuotaEntity(entityMap), Collections.singleton(operation)))).all().get();
            }
            Map<ClientQuotaEntity, Map<String, Double>> quotas = admin.describeClientQuotas(
                    ClientQuotaFilter.all()).entities().get();
            if (quotas.size() < 2
                    || !quotas.values().stream().anyMatch(values -> Double.valueOf(8.0).equals(values.get(QUOTA)))
                    || !quotas.values().stream().anyMatch(values -> Double.valueOf(1.0).equals(values.get(QUOTA)))) {
                throw new AssertionError("AlterClientQuotas did not preserve both principal overrides");
            }

            admin.deleteTopics(Collections.singletonList(primary)).all().get();
            if (admin.listTopics().names().get().contains(primary)) {
                throw new AssertionError("DeleteTopics left the topic visible");
            }
        }
        System.out.println("ADMIN CONFORMANCE OK");
    }

    private static void runDenied(String bootstrap) throws Exception {
        try (Admin admin = admin(bootstrap)) {
            try {
                admin.createTopics(Collections.singleton(new NewTopic(
                        "m12-admin-denied-" + System.nanoTime(), 1, (short) 1))).all().get();
            } catch (ExecutionException expected) {
                String description = String.valueOf(expected.getCause()).toLowerCase();
                if (description.contains("authorization")) {
                    System.out.println("ADMIN AUTHORIZATION DENIAL OK");
                    return;
                }
                throw new AssertionError("admin request failed for the wrong reason", expected);
            }
        }
        throw new AssertionError("data-plane listener accepted an admin mutation");
    }

    private static void runRoleDenied(String bootstrap) throws Exception {
        try (Admin admin = admin(bootstrap)) {
            try {
                admin.createTopics(Collections.singleton(new NewTopic(
                        "m12-admin-role-denied-" + System.nanoTime(), 1, (short) 1))).all().get();
            } catch (ExecutionException expected) {
                String description = String.valueOf(expected.getCause()).toLowerCase();
                if (description.contains("unsupportedversion")) {
                    System.out.println("ADMIN ROLE DENIAL OK");
                    return;
                }
                throw new AssertionError("role request failed for the wrong reason", expected);
            }
        }
        throw new AssertionError("data-plane listener accepted an admin mutation");
    }

    private static void runPersistenceCreate(String bootstrap) throws Exception {
        ConfigResource resource = new ConfigResource(ConfigResource.Type.TOPIC, "m12-admin-persist");
        try (Admin admin = admin(bootstrap)) {
            admin.createTopics(Collections.singleton(new NewTopic("m12-admin-persist", 1, (short) 1)))
                    .all().get();
            admin.alterConfigs(Collections.singletonMap(
                    resource, new Config(Collections.singletonList(new ConfigEntry(RETENTION, "86400000")))))
                    .all().get();
        }
        System.out.println("ADMIN PERSISTENCE WRITE OK");
    }

    private static void runPersistenceVerify(String bootstrap) throws Exception {
        ConfigResource resource = new ConfigResource(ConfigResource.Type.TOPIC, "m12-admin-persist");
        try (Admin admin = admin(bootstrap)) {
            if (!admin.listTopics().names().get().contains("m12-admin-persist")) {
                throw new AssertionError("topic did not survive broker restart");
            }
            Config config = admin.describeConfigs(Collections.singleton(resource)).all().get().get(resource);
            if (!"86400000".equals(config.get(RETENTION).value())) {
                throw new AssertionError("topic config did not survive broker restart");
            }
            admin.deleteTopics(Collections.singleton("m12-admin-persist")).all().get();
        }
        System.out.println("ADMIN PERSISTENCE VERIFY OK");
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            throw new IllegalArgumentException("usage: AdminConformance <bootstrap> <conformance|denied>");
        }
        if ("conformance".equals(args[1])) {
            runConformance(args[0]);
        } else if ("auth-denied".equals(args[1])) {
            runDenied(args[0]);
        } else if ("denied".equals(args[1])) {
            runRoleDenied(args[0]);
        } else if ("persistence-create".equals(args[1])) {
            runPersistenceCreate(args[0]);
        } else if ("persistence-verify".equals(args[1])) {
            runPersistenceVerify(args[0]);
        } else {
            throw new IllegalArgumentException("unknown mode: " + args[1]);
        }
    }
}
