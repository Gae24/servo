import pytest

from webdriver.bidi.error import MoveTargetOutOfBoundsException, NoSuchFrameException
from webdriver.bidi.modules.input import Actions, get_element_origin
from webdriver.bidi.modules.script import ContextTarget

from .. import get_events
from . import (
    assert_pointer_events,
    get_inview_center_bidi,
    get_shadow_root_from_test_page,
    record_pointer_events,
)

pytestmark = pytest.mark.asyncio

CONTEXT_LOAD_EVENT = "browsingContext.load"


async def test_pointer_down_closes_browsing_context(
    bidi_session, configuration, get_element, new_tab, inline, subscribe_events,
    wait_for_event
):
    url = inline("""<input onpointerdown="window.close()">close</input>""")

    # Opening a new context via `window.open` is required for script to be able
    # to close it.
    await subscribe_events(events=[CONTEXT_LOAD_EVENT])
    on_load = wait_for_event(CONTEXT_LOAD_EVENT)

    await bidi_session.script.evaluate(
        expression=f"window.open('{url}')",
        target=ContextTarget(new_tab["context"]),
        await_promise=True
    )
    # Wait for the new context to be created and get it.
    new_context = await on_load

    element = await get_element("input", context=new_context)
    origin = get_element_origin(element)

    actions = Actions()
    (
        actions.add_pointer(pointer_type="pen")
        .pointer_move(0, 0, origin=origin)
        .pointer_down(button=0)
        .pause(250 * configuration["timeout_multiplier"])
        .pointer_up(button=0)
    )

    with pytest.raises(NoSuchFrameException):
        await bidi_session.input.perform_actions(
            actions=actions, context=new_context["context"]
        )


@pytest.mark.parametrize("origin", ["element", "pointer", "viewport"])
async def test_params_actions_origin_outside_viewport(
    bidi_session, get_actions_origin_page, top_context, get_element, origin
):
    if origin == "element":
        url = get_actions_origin_page(
            """width: 100px; height: 50px; background: green;
            position: relative; left: -200px; top: -100px;"""
        )
        await bidi_session.browsing_context.navigate(
            context=top_context["context"],
            url=url,
            wait="complete",
        )

        element = await get_element("#inner")
        origin = get_element_origin(element)

    actions = Actions()
    (
        actions.add_pointer(pointer_type="pen")
        .pointer_move(x=-100, y=-100, origin=origin)
    )

    with pytest.raises(MoveTargetOutOfBoundsException):
        await bidi_session.input.perform_actions(
            actions=actions, context=top_context["context"]
        )


@pytest.mark.parametrize("mode", ["open", "closed"])
@pytest.mark.parametrize("nested", [False, True], ids=["outer", "inner"])
async def test_pen_pointer_in_shadow_tree(
    bidi_session, top_context, get_test_page, mode, nested
):
    await bidi_session.browsing_context.navigate(
        context=top_context["context"],
        url=get_test_page(
            shadow_doc="""
            <div id="pointer-target"
                 style="width: 10px; height: 10px; background-color:blue;">
            </div>""",
            shadow_root_mode=mode,
            nested_shadow_dom=nested,
        ),
        wait="complete",
    )

    shadow_root = await get_shadow_root_from_test_page(
        bidi_session, top_context, nested
    )

    # Add a simplified event recorder to track events in the test ShadowRoot.
    target = await record_pointer_events(
        bidi_session, top_context, shadow_root, "#pointer-target"
    )

    actions = Actions()
    (
        actions.add_pointer(pointer_type="pen")
        .pointer_move(x=0, y=0, origin=get_element_origin(target))
        .pointer_down(button=0)
        .pointer_up(button=0)
    )

    await bidi_session.input.perform_actions(
        actions=actions, context=top_context["context"]
    )

    await assert_pointer_events(
        bidi_session,
        top_context,
        expected_events=["pointerdown", "pointerup"],
        target="pointer-target",
        pointer_type="pen",
    )


async def test_pen_pointer_properties(
    bidi_session, top_context, get_element, load_static_test_page
):
    await load_static_test_page(page="test_actions_pointer.html")

    pointerArea = await get_element("#pointerArea")
    center = await get_inview_center_bidi(
        bidi_session, context=top_context, element=pointerArea
    )

    actions = Actions()
    (
        actions.add_pointer(pointer_type="pen")
        # Action 1. Move the pen to the pointerArea element. However, the
        # pen is not detected by digitizer yet.
        .pointer_move(x=0, y=0, origin=get_element_origin(pointerArea))
        # Action 2. Touch the pointerArea element with the pen.
        .pointer_down(button=0, pressure=0.36, altitude_angle=0.3,
                      azimuth_angle=0.2419, twist=86)
        # Action 3. Move the pen over the pointerArea to the right and down by
        # 10 pixels. The default pressure value is 0.5.
        .pointer_move(x=10, y=10, origin=get_element_origin(pointerArea))
        # # Action 4. Lift the pen from digitizer so that it cannot detect it
        # # anymore.
        .pointer_up(button=0)
        # # Action 5. Move the pen away from the pointerArea element. However,
        # the pen is not detected by digitizer anymore.
        .pointer_move(x=80, y=50, origin=get_element_origin(pointerArea))
    )

    await bidi_session.input.perform_actions(
        actions=actions, context=top_context["context"]
    )

    # Filter out noise "mouse" events, which can be generated by OS.
    events = [e for e in await get_events(bidi_session, top_context["context"])
              if e["pointerType"] == "pen"]

    event_types = [e["type"] for e in events]
    assert [
        # Action 1. Move the pen to the pointerArea element. No events are
        # expected, as the pen is not detected by digitizer yet.
        # Action 2. Touch the pointerArea element with the pen.
        "pointerover",
        "pointerenter",
        "pointerdown",
        # Action 3. Move the pen over the pointerArea.
        "pointermove",
        # Action 4. Lift the pen from digitizer.
        "pointerup",
        # Action 5. Move the pen away from the pointerArea element. No events
        # are expected, as the pen is not detected by digitizer anymore.
    ] == event_types

    assert events[2]["type"] == "pointerdown"
    assert events[2]["pageX"] == pytest.approx(center["x"], abs=1.0)
    assert events[2]["pageY"] == pytest.approx(center["y"], abs=1.0)
    assert events[2]["target"] == "pointerArea"
    assert events[2]["pointerType"] == "pen"
    # The default value of width and height for mouse and pen inputs is 1
    assert round(events[2]["width"], 2) == 1
    assert round(events[2]["height"], 2) == 1
    assert round(events[2]["pressure"], 2) == 0.36
    assert events[2]["tiltX"] == 72
    assert events[2]["tiltY"] == 38
    assert events[2]["twist"] == 86

    assert events[3]["type"] == "pointermove"
    assert events[3]["pageX"] == pytest.approx(center["x"] + 10, abs=1.0)
    assert events[3]["pageY"] == pytest.approx(center["y"] + 10, abs=1.0)
    assert events[3]["target"] == "pointerArea"
    assert events[3]["pointerType"] == "pen"
    assert round(events[3]["width"], 2) == 1
    assert round(events[3]["height"], 2) == 1
    assert round(events[3]["pressure"], 2) == 0.5
    assert events[3]["tiltX"] == 0
    assert events[3]["tiltY"] == 0
    assert events[3]["twist"] == 0

    assert events[4]["type"] == "pointerup"
    assert events[4]["pageX"] == pytest.approx(center["x"] + 10, abs=1.0)
    assert events[4]["pageY"] == pytest.approx(center["y"] + 10, abs=1.0)
    assert events[4]["target"] == "pointerArea"
    assert events[4]["pointerType"] == "pen"
    assert round(events[4]["width"], 2) == 1
    assert round(events[4]["height"], 2) == 1
    assert round(events[4]["pressure"], 2) == 0
    assert events[4]["tiltX"] == 0
    assert events[4]["tiltY"] == 0
    assert events[4]["twist"] == 0
