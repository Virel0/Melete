//! Orders chosen by hand: an episode one leaves out gets its rows back from one that has it.

use super::switching::{add_group_without_the_recap, download};
use super::*;

#[tokio::test]
async fn an_episode_left_out_and_brought_back_gets_its_rows_back() {
    let pool = some_anime().await;
    add_group_without_the_recap(&pool).await;
    download(&pool, 'a', 1, Some(13), "imported").await;
    download(&pool, 'b', 1, Some(14), "imported").await;
    let numbers = || async {
        let rows: Vec<(Option<i64>, Option<i64>, Option<i64>)> =
            sqlx::query_as("SELECT season, episode, left_out_tmdb_id FROM downloads ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        rows
    };
    choose(&pool, 100, Some("rrrr"), ChosenBy::Admin, 50)
        .await
        .unwrap();
    assert_eq!(
        numbers().await,
        vec![(None, None, Some(1013)), (Some(1), Some(13), None)]
    );
    // Back to TMDB's own order: the recap is S01E13 again, and the 14th S01E14.
    choose(&pool, 100, None, ChosenBy::Admin, 60).await.unwrap();
    assert_eq!(
        numbers().await,
        vec![(Some(1), Some(13), None), (Some(1), Some(14), None)]
    );
}
