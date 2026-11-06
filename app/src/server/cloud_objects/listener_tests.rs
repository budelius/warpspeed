use std::sync::Arc;

use cloud_object_client::MockObjectClient;
use warp_server_auth::credentials::Credentials;
use warpui::{App, SingletonEntity as _};

use super::Listener;
use crate::auth::AuthStateProvider;
use crate::auth::auth_manager::{AuthManager, AuthManagerEvent};
use crate::network::NetworkStatus;
use crate::server::cloud_objects::test_utils::initialize_app;
use crate::system::SystemStats;

fn initialize_listener_test_app(app: &mut App, logged_in: bool) {
    initialize_app(app);
    app.add_singleton_model(|_| SystemStats::new());

    if !logged_in {
        app.update(|ctx| {
            let auth_state = AuthStateProvider::as_ref(ctx).get();
            auth_state.set_user(None);
            auth_state.set_credentials(None);
        });
    }
}

#[test]
fn logged_out_listener_remembers_subscription_without_connecting() {
    App::test((), |mut app| async move {
        initialize_listener_test_app(&mut app, false);
        let mut object_client = MockObjectClient::new();
        object_client
            .expect_get_warp_drive_updates()
            .times(0)
            .returning(|_, _| Ok(()));
        let listener = app.add_singleton_model(|ctx| Listener::new(Arc::new(object_client), ctx));

        listener.update(&mut app, |listener, ctx| listener.start_listener(ctx));

        listener.read(&app, |listener, _| {
            assert!(listener.should_subscribe_to_updates);
            assert!(!listener.has_current_subscription_abort_handle());
        });
    });
}

#[test]
fn authentication_starts_the_deferred_subscription_once() {
    App::test((), |mut app| async move {
        initialize_listener_test_app(&mut app, false);
        let mut object_client = MockObjectClient::new();
        object_client
            .expect_get_warp_drive_updates()
            .times(1)
            .returning(|_, _| Ok(()));
        let listener = app.add_singleton_model(|ctx| Listener::new(Arc::new(object_client), ctx));

        listener.update(&mut app, |listener, ctx| listener.start_listener(ctx));
        app.update(|ctx| {
            AuthStateProvider::as_ref(ctx)
                .get()
                .set_credentials(Some(Credentials::Bearer("test-token".to_string())));
        });
        AuthManager::handle(&app).update(&mut app, |_, ctx| {
            ctx.emit(AuthManagerEvent::AuthComplete);
        });
        AuthManager::handle(&app).update(&mut app, |_, ctx| {
            ctx.emit(AuthManagerEvent::AuthComplete);
        });

        listener.read(&app, |listener, _| {
            assert!(listener.has_current_subscription_abort_handle());
        });
    });
}

#[test]
fn logged_out_listener_ignores_wake_and_network_recovery() {
    App::test((), |mut app| async move {
        initialize_listener_test_app(&mut app, true);
        let mut object_client = MockObjectClient::new();
        object_client
            .expect_get_warp_drive_updates()
            .times(1)
            .returning(|_, _| Ok(()));
        let listener = app.add_singleton_model(|ctx| Listener::new(Arc::new(object_client), ctx));

        listener.update(&mut app, |listener, ctx| listener.start_listener(ctx));
        app.update(|ctx| {
            AuthStateProvider::as_ref(ctx).get().set_credentials(None);
        });
        listener.update(&mut app, |listener, _| listener.stop_subscription());
        NetworkStatus::handle(&app).update(&mut app, |network, ctx| {
            network.reachability_changed(false, ctx);
            network.reachability_changed(true, ctx);
        });
        SystemStats::handle(&app).update(&mut app, |system, ctx| {
            system.dispatch_cpu_will_sleep(ctx);
            system.dispatch_cpu_was_awakened(ctx);
        });

        listener.read(&app, |listener, _| {
            assert!(listener.should_subscribe_to_updates);
            assert!(!listener.has_current_subscription_abort_handle());
        });
    });
}
