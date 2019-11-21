package com.tokera.ate.io.ram;

import com.tokera.ate.dao.MessageBundle;
import com.tokera.ate.dao.TopicAndPartition;
import com.tokera.ate.dao.msg.MessageBase;
import com.tokera.ate.dto.msg.MessageBaseDto;
import com.tokera.ate.dto.msg.MessageDataDto;
import com.tokera.ate.dto.msg.MessageDataMetaDto;
import com.tokera.ate.dto.msg.MessageMetaDto;
import com.tokera.ate.io.api.IPartitionKey;
import org.checkerframework.checker.nullness.qual.Nullable;
import org.javatuples.Pair;

import javax.enterprise.context.ApplicationScoped;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashSet;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.Set;

@ApplicationScoped
public class RamDataRepository {
    private final ConcurrentHashMap<TopicAndPartition, AtomicLong> offsets = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<TopicAndPartition, ArrayList<MessageBundle>> data = new ConcurrentHashMap<>();

    public MessageBundle write(TopicAndPartition where, String key, MessageBase msg) {
        long offset = offsets.computeIfAbsent(where, a -> new AtomicLong(0L)).incrementAndGet();

        MessageBundle bundle = new MessageBundle(key, where.partitionIndex(), offset, msg);
        data.compute(where, (k, l) -> {
            if (l == null) l = new ArrayList<>();
            l.add(bundle);
            return l;
        });
        return bundle;
    }

    public Iterable<MessageBundle> read(TopicAndPartition where) {
        return data.getOrDefault(where, new ArrayList<>());
    }

    public Iterable<MessageBundle> read(IPartitionKey key) {
        TopicAndPartition where = new TopicAndPartition(key);
        return data.getOrDefault(where, new ArrayList<>());
    }

    public @Nullable MessageDataMetaDto getVersion(TopicAndPartition where, long offset) {
        return data.getOrDefault(where, new ArrayList<>())
                .stream()
                .filter(a -> a.offset == offset)
                .filter(a -> a.partition == where.partitionIndex())
                .map(a -> new Pair<>(MessageBaseDto.from(a.raw), new MessageMetaDto(a.key, a.partition, a.offset)))
                .filter(a -> a.getValue0() instanceof MessageDataDto)
                .map(a -> new MessageDataMetaDto((MessageDataDto)a.getValue0(), a.getValue1()))
                .findFirst()
                .orElse(null);
    }

    public void deleteMany(TopicAndPartition where, Collection<String> keys) {
        HashSet<String> exists = new HashSet<>(keys);
        data.getOrDefault(where, new ArrayList<>())
                .removeIf(m -> exists.contains(m.key));
    }
}
