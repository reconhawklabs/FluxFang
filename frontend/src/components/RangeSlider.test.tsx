import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import RangeSlider from './RangeSlider';

const STOPS = [{ label: '100 ft' }, { label: '¼ mi' }, { label: '1 mi' }];

describe('RangeSlider', () => {
  it('shows the label of the current stop', () => {
    render(<RangeSlider label="Min distance" stops={STOPS} value={1} onChange={() => {}} />);
    expect(screen.getByText('¼ mi')).toBeInTheDocument();
  });

  it('emits the new index on change', () => {
    const onChange = vi.fn();
    render(<RangeSlider label="Min distance" stops={STOPS} value={1} onChange={onChange} />);
    fireEvent.change(screen.getByRole('slider'), { target: { value: '2' } });
    expect(onChange).toHaveBeenCalledWith(2);
  });
});
