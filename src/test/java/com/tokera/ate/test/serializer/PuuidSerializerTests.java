package com.tokera.ate.test.serializer;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.tokera.ate.dao.PUUID;
import com.tokera.ate.delegates.YamlDelegate;
import com.tokera.ate.enumerations.DataPartitionType;
import com.tokera.ate.extensions.YamlTagDiscoveryExtension;
import com.tokera.ate.io.api.IPartitionKey;
import com.tokera.ate.providers.PartitionKeySerializer;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestInstance;

import java.util.UUID;

@TestInstance(TestInstance.Lifecycle.PER_CLASS)
public class PuuidSerializerTests {
    private final static YamlTagDiscoveryExtension discovery = new YamlTagDiscoveryExtension();
    private final static YamlDelegate yamlDelegate = new YamlDelegate();

    static {
        yamlDelegate.init(discovery);
    }

    public class FakePartitionKey implements IPartitionKey {
        private String partitionTopic;
        private int partitionIndex;
        @JsonIgnore
        private transient String base64;

        public FakePartitionKey(String partitionTopic, int partitionIndex) {
            this.partitionTopic = partitionTopic;
            this.partitionIndex = partitionIndex;
        }

        @Override
        public String partitionTopic() {
            return partitionTopic;
        }

        @Override
        public int partitionIndex() {
            return partitionIndex;
        }

        @Override
        public DataPartitionType partitionType() { return DataPartitionType.Dao; }

        @Override
        public String toString() {
            return PartitionKeySerializer.toString(this);
        }

        @Override
        public int hashCode() {
            return PartitionKeySerializer.hashCode(this);
        }

        @Override
        public boolean equals(Object val) {
            return PartitionKeySerializer.equals(this, val);
        }

        @Override
        public String asBase64() {
            if (base64 != null) return base64;
            base64 = PartitionKeySerializer.serialize(this);
            return base64;
        }
    }

    //@Test
    public void yamlTest() {
        Test1Dto test = new Test1Dto();
        test.setShare(PUUID.from(new FakePartitionKey("testdomain.com", 1), UUID.randomUUID()));

        String yaml = yamlDelegate.serializeObj(test);
        Test1Dto test2 = (Test1Dto)yamlDelegate.deserializeObj(yaml);

        Assertions.assertEquals(test.getShare(), test2.getShare());
    }
}
