//! Small helpers with no home of their own.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

/// Runs every future concurrently on the caller's task and returns their
/// outputs in input order.
///
/// The fan-out futures hold a `Document` across await points, so they are not
/// `Send` and cannot be spawned; concurrency has to happen inside one task.
/// Every pending future is polled on each wake, which is correct (spurious
/// polls are allowed) and cheap for the handful of providers a fan-out has.
pub async fn join_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pending: Vec<Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = std::iter::repeat_with(|| None)
        .take(pending.len())
        .collect();
    let mut remaining = pending.len();

    std::future::poll_fn(move |cx| {
        for (index, future) in pending.iter_mut().enumerate() {
            if outputs[index].is_some() {
                continue;
            }
            if let Poll::Ready(value) = future.as_mut().poll(cx) {
                outputs[index] = Some(value);
                remaining -= 1;
            }
        }
        if remaining == 0 {
            Poll::Ready(
                std::mem::take(&mut outputs)
                    .into_iter()
                    .map(|output| output.expect("every future completed"))
                    .collect(),
            )
        } else {
            Poll::Pending
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn outputs_keep_the_input_order_whatever_the_completion_order() {
        let (first_tx, first_rx) = tokio::sync::oneshot::channel::<u32>();
        let (second_tx, second_rx) = tokio::sync::oneshot::channel::<u32>();

        let joined = join_all(vec![
            Box::pin(async move { first_rx.await.unwrap_or(0) })
                as std::pin::Pin<Box<dyn Future<Output = u32>>>,
            Box::pin(async move { second_rx.await.unwrap_or(0) }),
        ]);
        // The second future completes first.
        second_tx.send(2).expect("send");
        first_tx.send(1).expect("send");

        assert_eq!(joined.await, [1, 2]);
    }

    #[tokio::test]
    async fn an_empty_set_completes_immediately() {
        let outputs: Vec<u8> = join_all(Vec::<std::future::Ready<u8>>::new()).await;
        assert!(outputs.is_empty());
    }
}
