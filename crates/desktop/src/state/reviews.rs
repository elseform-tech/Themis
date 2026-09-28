use super::*;

impl AppState {
    /// Lists review items, optionally filtered by status, ordered by
    /// (`created_at`, `id`).
    pub async fn list_review_items(&self, status: Option<ReviewStatus>) -> Vec<ReviewItem> {
        let reviews = self.inner.reviews.read().await;
        let mut out: Vec<ReviewItem> = reviews
            .values()
            .filter(|item| status.is_none_or(|wanted| item.status == wanted))
            .cloned()
            .collect();
        out.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        out
    }

    /// Dismisses a review item, returning the updated item.
    pub async fn dismiss_review_item(&self, review_id: String) -> Result<ReviewItem, String> {
        let mut reviews = self.inner.reviews.write().await;
        let item = reviews
            .get_mut(&review_id)
            .ok_or_else(|| format!("unknown review item '{review_id}'"))?;
        item.status = ReviewStatus::Dismissed;
        let updated = item.clone();
        drop(reviews);
        self.persist_reviews().await;
        Ok(updated)
    }

    /// Marks a review item continued and returns its thread. The thread is
    /// resolved first so a missing thread does not change the review status.
    pub async fn continue_review_item(&self, review_id: String) -> Result<ThreadInfo, String> {
        let thread_id = {
            let reviews = self.inner.reviews.read().await;
            reviews
                .get(&review_id)
                .map(|item| item.thread_id.clone())
                .ok_or_else(|| format!("unknown review item '{review_id}'"))?
        };
        let info = self.get_thread(&thread_id).await?;
        {
            let mut reviews = self.inner.reviews.write().await;
            if let Some(item) = reviews.get_mut(&review_id) {
                item.status = ReviewStatus::Continued;
            }
        }
        self.persist_reviews().await;
        Ok(info)
    }
}
