package com.tokera.examples.dao;

import com.tokera.ate.annotations.PermitParentType;
import com.tokera.ate.dao.PUUID;
import com.tokera.ate.dao.base.BaseDaoRights;
import com.tokera.ate.units.DaoId;
import org.checkerframework.checker.nullness.qual.Nullable;

import java.math.BigDecimal;
import java.util.Date;
import java.util.UUID;

@PermitParentType(MonthlyActivity.class)
public class TransactionDetails extends BaseDaoRights {
    public UUID id;
    public UUID monthlyActivity;
    public BigDecimal amount;
    public Date when;
    @Nullable
    public String details;
    public PUUID mirroredTransaction;

    @SuppressWarnings("initialization.fields.uninitialized")
    @Deprecated
    public TransactionDetails() {
    }

    public TransactionDetails(MonthlyActivity monthly, BigDecimal amount, PUUID mirroredTransaction) {
        this.id = UUID.randomUUID();
        this.monthlyActivity = monthly.id;
        this.amount = amount;
        this.mirroredTransaction = mirroredTransaction;
        this.when = new Date();
    }

    public @DaoId UUID getId() {
        return this.id;
    }

    public @Nullable @DaoId UUID getParentId() {
        return this.monthlyActivity;
    }
}
