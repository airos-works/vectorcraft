#!/usr/bin/env python3
"""Original Haiku VectorCraft icon: an artboard with a Bezier curve, its handles and a pen nib
(MIT OR Apache-2.0). Drawn here from scratch in upstream's orange and cream; no artwork is
traced or copied."""
from icon_art import Drawing, INK, plane

d = Drawing()
_plane = plane(5, 9, 0.3)


def p(points):
    # Rounded, so that each coordinate survives HVIF's encoding as it is.
    return [(round(x, 3), round(y, 3)) for x, y in _plane(points)]


d.shadow([(5,46),(44,58),(62,58),(30,46)])
# The artboard, a cream sheet receding to the right.
d.shape(p([(0,0),(40,0),(40,38),(0,38)]), d.gradient('fffbf2','dcd3c2'), INK)
# The curve: an S from the lower left to the upper right, drawn in the app's orange.
curve = [tuple(p([q])[0] for q in pt) for pt in [((4,30),(4,30),(10,14)), ((20,20),(14,26),(26,12)), ((36,8),(30,4),(36,8))]]
d.shape(curve, None, 'e8573f', 2.6, closed=False)
# Handles of the middle anchor, and the three anchors as squares.
d.line(p([(13,27),(27,13)]), '657184', 1.2)
for x, y in p([(13,27),(27,13)]):
    d.ellipse(x, y, 1.6, 1.6, 'ffffff', '465061', 1.0)
for x, y in p([(4,30),(20,20),(36,8)]):
    d.shape([(round(x+u, 3), round(y+v, 3)) for u, v in ((-2.2,-2.2),(2.2,-2.2),(2.2,2.2),(-2.2,2.2))], 'ffffff', INK, 1.4)
# A fountain-pen nib coming in from the lower right, its tip at the curve's end.
d.shape([(42,31),(43,42),(48,47),(59,36),(54,31)], d.gradient('f58a63','b63a24', (44,32),(58,46)), INK, 1.6)
d.line([(42,31),(48.5,37.5)], '6e2012', 1.2)
d.ellipse(49, 38, 1.8, 1.8, 'fff4e8', INK, 1.0)
d.shape([(48,47),(59,36),(63,40),(52,51)], d.gradient('6b7586','262b33', (50,40),(60,50)), INK, 1.6)
d.save(__file__, 'vectorcraft', 'VectorCraft')
