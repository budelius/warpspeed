use warpui::{AppContext, ModelHandle, SingletonEntity, View, ViewContext, ViewHandle};

use super::view::PaneView;
use super::{
    DetachType, PaneConfiguration, PaneContent, PaneGroup, PaneId, ShareableLink,
    ShareableLinkError,
};
use crate::agent_ctrl::FleetPaneManager;
use crate::agent_ctrl::fleet_model::FleetModel;
use crate::agent_ctrl::fleet_view::{FleetView, FleetViewEvent};
use crate::app_state::LeafContents;
use crate::workspace::PaneViewLocator;

pub struct AgentCtrlFleetPane {
    view: ViewHandle<PaneView<FleetView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl AgentCtrlFleetPane {
    pub fn from_view(fleet_view: ViewHandle<FleetView>, ctx: &mut AppContext) -> Self {
        let pane_configuration = fleet_view.as_ref(ctx).pane_configuration();

        let view = ctx.add_typed_action_view(fleet_view.window_id(ctx), |ctx| {
            let pane_id = PaneId::from_agent_ctrl_fleet_pane_ctx(ctx);
            PaneView::new(pane_id, fleet_view, (), pane_configuration.clone(), ctx)
        });

        Self {
            view,
            pane_configuration,
        }
    }

    pub fn new<V: View>(ctx: &mut ViewContext<V>) -> Self {
        let view = ctx.add_typed_action_view(FleetView::new);
        Self::from_view(view, ctx)
    }

    pub fn fleet_view(&self, ctx: &AppContext) -> ViewHandle<FleetView> {
        self.view.as_ref(ctx).child(ctx)
    }
}

impl PaneContent for AgentCtrlFleetPane {
    fn id(&self) -> PaneId {
        PaneId::from_agent_ctrl_fleet_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: crate::pane_group::focus_state::PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        let fleet_view = self.fleet_view(ctx);
        let pane_id = self.id();
        let pane_group_id = ctx.view_id();
        let window_id = ctx.window_id();

        ctx.subscribe_to_view(&fleet_view, move |pane_group, _, event, ctx| {
            let FleetViewEvent::Pane(pane_event) = event;
            pane_group.handle_pane_event(pane_id, pane_event, ctx)
        });
        ctx.subscribe_to_view(&self.view, move |group, _, event, ctx| {
            group.handle_pane_view_event(pane_id, event, ctx);
        });

        FleetPaneManager::handle(ctx).update(ctx, |manager, _| {
            manager.register_pane(
                window_id,
                PaneViewLocator {
                    pane_group_id,
                    pane_id,
                },
            );
        });
        FleetModel::handle(ctx).update(ctx, |model, ctx| model.attach_view(ctx));
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let fleet_view = self.fleet_view(ctx);
        ctx.unsubscribe_to_view(&fleet_view);
        ctx.unsubscribe_to_view(&self.view);

        let window_id = ctx.window_id();
        FleetPaneManager::handle(ctx).update(ctx, |manager, _| {
            manager.deregister_pane(&window_id);
        });
        FleetModel::handle(ctx).update(ctx, |model, _| model.detach_view());
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::AgentCtrlFleet
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.fleet_view(ctx)
            .update(ctx, |view, ctx| view.focus(ctx));
    }

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        Ok(ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}
