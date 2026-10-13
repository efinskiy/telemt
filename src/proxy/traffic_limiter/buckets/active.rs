use super::*;

impl CidrUserDirectionState {
    /// Registers each static CIDR user at most once per epoch.
    pub(in super::super) fn ensure_active(
        &self,
        epoch: u64,
        active_users: &Arc<DirectionBucket>,
        budget: &mut ReserveCasBudget,
    ) -> Result<bool, BucketReserveError> {
        self.ensure_active_inner(epoch, active_users, budget, 1, false)
    }

    /// Publishes a bounded weighted IP credit once per epoch without multiplying it by flow count.
    pub(in super::super) fn ensure_active_weighted(
        &self,
        epoch: u64,
        active_users: &Arc<DirectionBucket>,
        budget: &mut ReserveCasBudget,
        weight: u64,
    ) -> Result<bool, BucketReserveError> {
        self.ensure_active_inner(epoch, active_users, budget, weight.clamp(1, 16), true)
    }

    fn ensure_active_inner(
        &self,
        epoch: u64,
        active_users: &Arc<DirectionBucket>,
        budget: &mut ReserveCasBudget,
        weight: u64,
        stamp_weight: bool,
    ) -> Result<bool, BucketReserveError> {
        if epoch > PACKED_EPOCH_MAX {
            return Err(BucketReserveError::StaleEpoch);
        }
        let mut observed = self.used.state.load(Ordering::Relaxed);
        // Every retry spends shared reserve budget before publishing the user epoch.
        loop {
            let (observed_epoch, _) = DirectionBucket::unpack(observed);
            if observed_epoch == epoch {
                return Ok(true);
            }
            if observed_epoch > epoch {
                return Err(BucketReserveError::StaleEpoch);
            }
            let Some(mut active_debit) = active_users.try_reserve_at(
                epoch,
                PACKED_USAGE_MASK,
                weight.clamp(1, 16),
                budget,
            )?
            else {
                return Ok(false);
            };
            let Some(next) = DirectionBucket::pack(epoch, if stamp_weight { weight } else { 0 })
            else {
                return Err(BucketReserveError::StaleEpoch);
            };
            if !budget.take() {
                if active_debit.refund_all() == BucketRefundOutcome::Contended {
                    return Err(BucketReserveError::ReserveAndRefundContended);
                }
                return Err(BucketReserveError::Contended);
            }
            match self.used.compare_exchange_reserve(observed, next) {
                Ok(_) => {
                    active_debit.commit_all();
                    return Ok(true);
                }
                Err(actual) => {
                    let refund = active_debit.refund_all();
                    let (actual_epoch, _) = DirectionBucket::unpack(actual);
                    if refund == BucketRefundOutcome::Contended {
                        return Err(if budget.is_exhausted() {
                            BucketReserveError::ReserveAndRefundContended
                        } else {
                            BucketReserveError::RefundContended
                        });
                    }
                    if actual_epoch == epoch {
                        return Ok(true);
                    }
                    observed = actual;
                }
            }
        }
    }
}
