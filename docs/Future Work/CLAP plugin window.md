### work plugin windows are

It’s about two to three times the work of the CLAP host so far: roughly
1,500–2,500 lines over a few long sessions. The window itself is the small
part; the pieces around it are the real work.

1.  **Threading rework (medium).** CLAP requires a plugin’s window and the rest
    of its lifecycle to run on one thread, and on macOS windows must be on the
    app’s main thread. Today plugins live on their own thread. The UI’s event
    loop would have to run plugin jobs, with care so the mixer and UI never
    wait on each other and deadlock.
2.  **One instance, two users (medium-hard).** The window has to attach to the
    very instance making the sound, which lives inside the mixer, per clip.
    That needs a shared table of live instances that both the mixer and the UI
    can reach. Exports use a separate mixer, so they need a copy of each
    plugin’s state.
3.  **The window itself (medium on macOS).** Open a native window, hand the
    plugin its view, and handle sizing, resizing, Retina scale, close, and the
    plugin asking to show or hide. Windows and Linux are similar code, but I
    can’t test them without those machines.
4.  **Edits from the plugin’s window become project edits (medium).** When you
    turn a knob in the plugin’s own window, the change has to come back from
    the audio thread and become an undoable edit, with a whole knob drag
    counting as one undo step. Effect Controls and keyframes stay in sync both
    ways.
5.  **Saving plugin state (medium).** Settings that aren’t parameters
    (Airwindows’ choice of effect, presets, loaded files) need to be stored in
    the project. That’s a project file format change. It’s also what makes
    Airwindows actually usable.
6.  **UI and lifetime (small).** An “Edit…” button on each plugin effect in
    Effect Controls. Windows close when the effect, clip or project goes away.

**Testing:** mostly by hand with Equilibrium, Meridian and Airwindows, which all
have windows. I can automate the state and parameter round trips, but not the
windows themselves.

**Suggested order:**

- **A:** state saving and edits from plugin windows (items 4–5). Useful on its
  own: Airwindows becomes fully usable once its chosen effect can be saved.
- **B:** windows on macOS (items 1–3 and 6). This is where you see the plugin’s
  interface.
- **C:** Windows and Linux windows, written but untested until CI or a PC.
