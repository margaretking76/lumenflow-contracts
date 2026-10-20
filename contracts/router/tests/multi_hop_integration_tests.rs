//! Multi-hop payment routing integration tests for the LumenFlow router contract.
//!
//! Resolves issue #1326: Add router contract integration tests for multi-hop
//! payment routing.
//!
//! Covers:
//!   - Successful 2-hop route through two active merchants
//!   - Route through an inactive merchant (should fail with MerchantNotActive)
//!   - Route exceeding MAX_HOPS (should fail with TooManyHops)
//!   - Empty route list (should fail with EmptyRoute)

extern crate alloc;

use soroban_sdk::{testutils::Address as _, vec, Address, Env, String};

use lumenflow::{MerchantCategory, PaymentProcessingContract, PaymentProcessingContractClient};
use lumenflow_router::{
    RouterContract, RouterContractClient, RouterError, RouteHop, MAX_HOPS,
};

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Shared fixture: deploys both the lumenflow and router contracts, registers
/// two active merchants and one inactive merchant, and wires the router to the
/// lumenflow instance.
///
/// Returns `(env, router_client, lumenflow_client, admin, merchant_a,
///           merchant_b, inactive_merchant, token)`.
fn setup_multi_hop_env() -> (
    Env,
    RouterContractClient<'static>,
    PaymentProcessingContractClient<'static>,
    Address,
    Address,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    // Deploy lumenflow contract
    let lumenflow_id = env.register(PaymentProcessingContract, ());
    let lumenflow_client = PaymentProcessingContractClient::new(&env, &lumenflow_id);

    // Deploy router contract
    let router_id = env.register(RouterContract, ());
    let router_client = RouterContractClient::new(&env, &router_id);

    let admin = Address::generate(&env);
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);
    let inactive_merchant = Address::generate(&env);

    // Create a test SAC token
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();

    // Initialise lumenflow
    lumenflow_client.set_admin(&admin);
    lumenflow_client.add_allowed_token(&admin, &token);

    // Register merchant_a (active)
    lumenflow_client.register_merchant(
        &merchant_a,
        &String::from_str(&env, "Merchant A"),
        &String::from_str(&env, "First hop merchant"),
        &String::from_str(&env, "a@merchant.com"),
        &MerchantCategory::Retail,
    );

    // Register merchant_b (active)
    lumenflow_client.register_merchant(
        &merchant_b,
        &String::from_str(&env, "Merchant B"),
        &String::from_str(&env, "Second hop merchant"),
        &String::from_str(&env, "b@merchant.com"),
        &MerchantCategory::Retail,
    );

    // Register inactive_merchant and then deactivate it
    lumenflow_client.register_merchant(
        &inactive_merchant,
        &String::from_str(&env, "Inactive Merchant"),
        &String::from_str(&env, "A deactivated merchant"),
        &String::from_str(&env, "inactive@merchant.com"),
        &MerchantCategory::Retail,
    );
    lumenflow_client.deactivate_merchant(&admin, &inactive_merchant);

    // Wire the router to the lumenflow contract for merchant validation
    router_client.set_lumenflow_contract(&lumenflow_id);

    (
        env,
        router_client,
        lumenflow_client,
        admin,
        merchant_a,
        merchant_b,
        inactive_merchant,
        token,
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// #1326-AC1: Successful 2-hop route through two active merchants.
///
/// A route with two hops, each targeting an active and registered merchant,
/// should return a `MultiHopResult` with `hop_count == 2` and preserve the
/// hop data exactly as supplied.
#[test]
fn test_multi_hop_two_active_merchants_succeeds() {
    let (env, router, _lumenflow, _admin, merchant_a, merchant_b, _inactive, token) =
        setup_multi_hop_env();

    let hops = vec![
        &env,
        RouteHop {
            merchant: merchant_a.clone(),
            token: token.clone(),
            amount: 1_000_i128,
        },
        RouteHop {
            merchant: merchant_b.clone(),
            token: token.clone(),
            amount: 500_i128,
        },
    ];

    let result = router.execute_multi_hop_route(&hops);
    assert_eq!(result.hop_count, 2, "expected 2 hops to be executed");
    assert_eq!(
        result.hops.get(0).unwrap().merchant,
        merchant_a,
        "first hop merchant must be merchant_a"
    );
    assert_eq!(
        result.hops.get(1).unwrap().merchant,
        merchant_b,
        "second hop merchant must be merchant_b"
    );
    assert_eq!(
        result.hops.get(0).unwrap().amount,
        1_000_i128,
        "first hop amount must be preserved"
    );
    assert_eq!(
        result.hops.get(1).unwrap().amount,
        500_i128,
        "second hop amount must be preserved"
    );
}

/// #1326-AC2: Route through an inactive merchant must fail with MerchantNotActive.
///
/// When one of the hops targets a merchant that is registered but deactivated,
/// `execute_multi_hop_route` must return `RouterError::MerchantNotActive`.
#[test]
fn test_multi_hop_through_inactive_merchant_returns_merchant_not_active() {
    let (env, router, _lumenflow, _admin, merchant_a, _merchant_b, inactive_merchant, token) =
        setup_multi_hop_env();

    let hops = vec![
        &env,
        RouteHop {
            merchant: merchant_a.clone(),
            token: token.clone(),
            amount: 1_000_i128,
        },
        RouteHop {
            merchant: inactive_merchant.clone(),
            token: token.clone(),
            amount: 500_i128,
        },
    ];

    let result = router.try_execute_multi_hop_route(&hops);
    assert_eq!(
        result,
        Err(Ok(RouterError::MerchantNotActive)),
        "expected MerchantNotActive when routing through a deactivated merchant"
    );
}

/// #1326-AC3: Route that exceeds MAX_HOPS must fail with TooManyHops.
///
/// Attempting to route through more than `MAX_HOPS` merchants must be rejected
/// before any on-chain state is read, returning `RouterError::TooManyHops`.
#[test]
fn test_multi_hop_exceeds_max_hops_returns_too_many_hops() {
    let (env, router, _lumenflow, _admin, merchant_a, _merchant_b, _inactive, token) =
        setup_multi_hop_env();

    // Build a route with MAX_HOPS + 1 hops (all pointing to merchant_a)
    let over_limit = (MAX_HOPS + 1) as usize;
    let mut hop_vec = soroban_sdk::vec![&env];
    for _ in 0..over_limit {
        hop_vec.push_back(RouteHop {
            merchant: merchant_a.clone(),
            token: token.clone(),
            amount: 100_i128,
        });
    }

    let result = router.try_execute_multi_hop_route(&hop_vec);
    assert_eq!(
        result,
        Err(Ok(RouterError::TooManyHops)),
        "expected TooManyHops when route exceeds MAX_HOPS ({})",
        MAX_HOPS
    );
}

/// #1326-AC4: Empty route list must fail with EmptyRoute.
///
/// Submitting an empty hop list to `execute_multi_hop_route` must be rejected
/// immediately with `RouterError::EmptyRoute`.
#[test]
fn test_multi_hop_empty_route_returns_empty_route_error() {
    let (env, router, ..) = setup_multi_hop_env();

    let empty_hops: soroban_sdk::Vec<RouteHop> = soroban_sdk::vec![&env];

    let result = router.try_execute_multi_hop_route(&empty_hops);
    assert_eq!(
        result,
        Err(Ok(RouterError::EmptyRoute)),
        "expected EmptyRoute when the hop list is empty"
    );
}

/// Extra: validate_route with a zero-amount hop returns InvalidAmount.
#[test]
fn test_validate_route_zero_amount_returns_invalid_amount() {
    let (env, router, _lumenflow, _admin, merchant_a, _merchant_b, _inactive, token) =
        setup_multi_hop_env();

    let hops = vec![
        &env,
        RouteHop {
            merchant: merchant_a.clone(),
            token: token.clone(),
            amount: 0_i128,
        },
    ];

    let result = router.try_validate_route(&hops);
    assert_eq!(
        result,
        Err(Ok(RouterError::InvalidAmount)),
        "expected InvalidAmount for a hop with amount == 0"
    );
}

/// Extra: route to an unregistered merchant returns MerchantNotFound.
#[test]
fn test_multi_hop_unregistered_merchant_returns_merchant_not_found() {
    let (env, router, ..) = setup_multi_hop_env();

    let unknown = Address::generate(&env);
    // Use a token from the already-configured env (re-register one)
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let hops = vec![
        &env,
        RouteHop {
            merchant: unknown,
            token,
            amount: 100_i128,
        },
    ];

    let result = router.try_execute_multi_hop_route(&hops);
    assert_eq!(
        result,
        Err(Ok(RouterError::MerchantNotFound)),
        "expected MerchantNotFound for an address never registered as a merchant"
    );
}

/// Extra: MAX_HOPS exact boundary — a route with exactly MAX_HOPS hops succeeds.
#[test]
fn test_multi_hop_exactly_max_hops_succeeds() {
    let (env, router, _lumenflow, _admin, merchant_a, _merchant_b, _inactive, token) =
        setup_multi_hop_env();

    let mut hop_vec = soroban_sdk::vec![&env];
    for _ in 0..MAX_HOPS {
        hop_vec.push_back(RouteHop {
            merchant: merchant_a.clone(),
            token: token.clone(),
            amount: 100_i128,
        });
    }

    let result = router.execute_multi_hop_route(&hop_vec);
    assert_eq!(
        result.hop_count, MAX_HOPS,
        "expected exactly {} hops to succeed",
        MAX_HOPS
    );
}
